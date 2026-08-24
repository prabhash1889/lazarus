//! Resumable HTTP downloads with persistent partial state.
//!
//! Partial data lives at `<dest>.part` with a sidecar `<dest>.part.json`
//! describing what the partial bytes belong to (expected digest and total
//! size). A later attempt with the same expectations resumes via a
//! `Range: bytes=<len>-` request; anything else (mismatched metadata, a
//! server that ignores Range, an unexpected response) restarts from zero.
//! The file is only reported complete after its size and SHA-256 both
//! match the signed manifest, so a corrupted or truncated transfer can
//! never reach staging.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Everything needed to fetch and fully verify one artifact.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    /// Final resting place of the completed artifact. While downloading,
    /// `<destination>.part` is used instead.
    pub destination: PathBuf,
    /// Lowercase hex SHA-256 the completed file must have.
    pub expected_sha256: String,
    pub expected_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialMeta {
    expected_sha256: String,
    expected_size: u64,
}

pub struct CompletedDownload {
    pub path: PathBuf,
    pub sha256: String,
    pub resumed_from: u64,
}

fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .expect("destination has a file name")
        .to_os_string();
    name.push(".part");
    destination.with_file_name(name)
}

fn meta_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .expect("destination has a file name")
        .to_os_string();
    name.push(".part.json");
    destination.with_file_name(name)
}

/// Downloads `request` fully, resuming any compatible partial from a prior
/// interrupted run. Returns the verified artifact path on success.
pub async fn download_resumable(
    client: &reqwest::Client,
    request: &DownloadRequest,
) -> Result<CompletedDownload> {
    let part = part_path(&request.destination);
    let meta = meta_path(&request.destination);
    let expected = PartialMeta {
        expected_sha256: normalize_digest(&request.expected_sha256),
        expected_size: request.expected_size,
    };

    let existing = load_compatible_partial(&meta, &part, &expected)?;
    persist_partial_meta(request)?;
    if existing == expected.expected_size {
        // A previous run finished writing but died before verification;
        // skip straight to it instead of asking the server for zero bytes.
        return verify_and_publish(request, &part, &meta, 0);
    }

    let mut request_builder = client.get(&request.url);
    let mut resumed_from = 0u64;
    let append = existing > 0;
    if append {
        resumed_from = existing;
        request_builder =
            request_builder.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }

    let response = request_builder
        .send()
        .await
        .with_context(|| format!("requesting {}", request.url))?;

    let status = response.status();
    let usable_range = status == reqwest::StatusCode::PARTIAL_CONTENT && append;
    let fresh_start = status == reqwest::StatusCode::OK || (!append && status.is_success());
    if !usable_range && !fresh_start {
        bail!(
            "download server answered {} for {}",
            status.as_u16(),
            request.url
        );
    }
    if !usable_range {
        // The server ignored our Range or we had no partial: start over.
        resumed_from = 0;
        fs::remove_file(&part).ok();
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(usable_range)
        .write(true)
        .truncate(!usable_range)
        .open(&part)
        .with_context(|| format!("opening {}", part.display()))?;

    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .with_context(|| format!("connection dropped while downloading {}", request.url))?
    {
        file.write_all(&chunk)
            .with_context(|| format!("writing {}", part.display()))?;
    }
    file.sync_all().ok();
    drop(file);

    verify_and_publish(request, &part, &meta, resumed_from)
}

/// Verifies the completed part against the manifest and moves it into
/// place, removing the partial-tracking sidecar.
fn verify_and_publish(
    request: &DownloadRequest,
    part: &Path,
    meta: &Path,
    resumed_from: u64,
) -> Result<CompletedDownload> {
    let actual_size = fs::metadata(part)
        .map(|metadata| metadata.len())
        .with_context(|| format!("checking {}", part.display()))?;
    if actual_size != request.expected_size {
        bail!(
            "downloaded artifact is {} bytes but the signed manifest says {}",
            actual_size,
            request.expected_size
        );
    }
    let actual_digest = hash_file(part)?;
    let expected_digest = normalize_digest(&request.expected_sha256);
    if actual_digest != expected_digest {
        // The partial state is worthless now; drop it so the next attempt
        // starts clean instead of compounding corruption.
        fs::remove_file(part).ok();
        fs::remove_file(meta).ok();
        bail!(
            "downloaded artifact checksum mismatch: expected sha256 {expected_digest}, got {actual_digest}"
        );
    }

    if let Some(parent) = request.destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    // Persist-then-move keeps the contract honest: the meta only
    // disappears once the full file exists under its real name.
    fs::rename(part, &request.destination)
        .with_context(|| format!("publishing {}", request.destination.display()))?;
    fs::remove_file(meta).ok();

    Ok(CompletedDownload {
        path: request.destination.clone(),
        sha256: expected_digest,
        resumed_from,
    })
}

/// Returns how many bytes of a still-valid partial exist. Incompatible or
/// oversized partials are discarded rather than resumed.
fn load_compatible_partial(
    meta_path: &Path,
    part_path: &Path,
    expected: &PartialMeta,
) -> Result<u64> {
    if !meta_path.exists() || !part_path.exists() {
        discard_orphaned(meta_path, part_path);
        return Ok(0);
    }
    let outcome = (|| -> Result<PartialMeta> {
        let raw = fs::read_to_string(meta_path)
            .with_context(|| format!("reading {}", meta_path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", meta_path.display()))
    })();
    match outcome {
        Ok(stored)
            if stored.expected_sha256 == expected.expected_sha256
                && stored.expected_size == expected.expected_size =>
        {
            let len = fs::metadata(part_path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if len > expected.expected_size {
                reset_partial(meta_path, part_path)?;
                return Ok(0);
            }
            Ok(len)
        }
        _ => {
            reset_partial(meta_path, part_path)?;
            Ok(0)
        }
    }
}

/// Writes the sidecar describing in-flight partial bytes so an
/// interrupted process (or a later CLI invocation) can resume.
pub fn persist_partial_meta(request: &DownloadRequest) -> Result<()> {
    let meta = meta_path(&request.destination);
    if let Some(parent) = meta.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let stored = PartialMeta {
        expected_sha256: normalize_digest(&request.expected_sha256),
        expected_size: request.expected_size,
    };
    let raw =
        serde_json::to_string_pretty(&stored).context("encoding the partial-download record")?;
    File::create(&meta)
        .and_then(|mut file| file.write_all(raw.as_bytes()))
        .with_context(|| format!("writing {}", meta.display()))?;
    Ok(())
}

fn reset_partial(meta_path: &Path, part_path: &Path) -> Result<()> {
    fs::remove_file(meta_path).ok();
    fs::remove_file(part_path)
        .with_context(|| format!("discarding incompatible partial {}", part_path.display()))
}

fn discard_orphaned(meta_path: &Path, part_path: &Path) {
    fs::remove_file(meta_path).ok();
    fs::remove_file(part_path).ok();
}

pub fn normalize_digest(digest: &str) -> String {
    digest.trim().to_ascii_lowercase()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(Sha256::digest(bytes).as_slice())
}

/// Streams `path` through SHA-256 without loading it into memory.
pub fn hash_file(path: &Path) -> Result<String> {
    use std::io::Read;

    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(hasher.finalize().as_slice()))
}

pub(crate) fn require_digest_match(expected_hex: &str, actual_hex: &str) -> Result<()> {
    if normalize_digest(expected_hex) != normalize_digest(actual_hex) {
        return Err(anyhow!(
            "artifact checksum mismatch: expected sha256 {}, got {}",
            normalize_digest(expected_hex),
            normalize_digest(actual_hex)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "lazarus-download-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    const PAYLOAD: &[u8] = b"an artifact body that is never trusted until hashed";

    fn request_for(dir: &Path, url: &str) -> DownloadRequest {
        DownloadRequest {
            url: url.to_owned(),
            destination: dir.join("artifact"),
            expected_sha256: sha256_hex(PAYLOAD),
            expected_size: PAYLOAD.len() as u64,
        }
    }

    #[test]
    fn partial_meta_describes_the_expected_transfer() {
        let dir = temp_root("meta");
        let request = request_for(&dir, "http://127.0.0.1:1/rel");
        persist_partial_meta(&request).expect("meta written");

        let meta = dir.join("artifact.part.json");
        assert!(meta.exists());
        let stored: PartialMeta =
            serde_json::from_str(&fs::read_to_string(&meta).expect("read")).expect("valid");
        assert_eq!(stored.expected_size, PAYLOAD.len() as u64);
        assert_eq!(stored.expected_sha256, sha256_hex(PAYLOAD));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn incompatible_partials_are_discarded_not_resumed() {
        let dir = temp_root("reset");
        let request = request_for(&dir, "http://127.0.0.1:1/rel");
        persist_partial_meta(&request).expect("meta written");
        fs::write(dir.join("artifact.part"), b"some partial bytes").expect("partial");

        // A later attempt for a different release (different digest):
        // neither the sidecar nor its partial may be reused.
        let other_release = PartialMeta {
            expected_sha256: "f".repeat(64),
            expected_size: request.expected_size,
        };
        let existing = load_compatible_partial(
            &dir.join("artifact.part.json"),
            &dir.join("artifact.part"),
            &other_release,
        )
        .expect("evaluated");
        assert_eq!(existing, 0, "mismatched partials restart from zero");
        assert!(
            !dir.join("artifact.part").exists(),
            "the stale partial is discarded"
        );
        assert!(!dir.join("artifact.part.json").exists());

        // An oversized partial (more bytes than the manifest allows) too.
        persist_partial_meta(&request).expect("meta rewritten");
        fs::write(
            dir.join("artifact.part"),
            vec![0u8; request.expected_size as usize + 1],
        )
        .expect("oversized partial");
        let expected = PartialMeta {
            expected_sha256: request.expected_sha256.clone(),
            expected_size: request.expected_size,
        };
        let existing = load_compatible_partial(
            &dir.join("artifact.part.json"),
            &dir.join("artifact.part"),
            &expected,
        )
        .expect("evaluated");
        assert_eq!(existing, 0);
        assert!(!dir.join("artifact.part").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn matching_artifact_resumes_from_a_different_mirror() {
        let dir = temp_root("mirror");
        let first = request_for(&dir, "http://127.0.0.1:1/rel");
        persist_partial_meta(&first).expect("meta written");
        fs::write(dir.join("artifact.part"), b"some partial bytes").expect("partial");

        let second = request_for(&dir, "http://127.0.0.1:2/rel");
        let expected = PartialMeta {
            expected_sha256: normalize_digest(&second.expected_sha256),
            expected_size: second.expected_size,
        };
        assert_eq!(
            load_compatible_partial(
                &dir.join("artifact.part.json"),
                &dir.join("artifact.part"),
                &expected,
            )
            .expect("evaluated"),
            b"some partial bytes".len() as u64
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn file_hashing_matches_direct_hashing_and_case_is_normalized() {
        let dir = temp_root("hash");
        let path = dir.join("blob");
        fs::write(&path, PAYLOAD).expect("payload");
        assert_eq!(hash_file(&path).expect("hashed"), sha256_hex(PAYLOAD));
        let digest = sha256_hex(PAYLOAD);
        assert!(require_digest_match(&digest.to_uppercase(), &digest).is_ok());
        assert!(require_digest_match(&"0".repeat(64), &digest).is_err());
        fs::remove_dir_all(dir).ok();
    }
}
