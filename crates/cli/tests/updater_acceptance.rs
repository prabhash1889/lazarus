//! Real-flow acceptance checks for the Phase 2.4 CLI-owned Host updater.
//!
//! Exit gate: reject an invalid signature or checksum, resume an
//! interrupted download, promote a valid update atomically, and
//! explicitly roll back to the retained installation.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ed25519_dalek::{SigningKey, VerifyingKey};
use lazarus_cli::updater::layout::InstallPaths;
use lazarus_cli::updater::manifest::{Artifact, Release, ReleaseManifest, sign_manifest};
use lazarus_cli::updater::{ApplyOutcome, Updater};
use lazarus_hostd::runtime::DataPaths;

/// Deterministic throwaway release keys for the test signer and the
/// pinned verifier the updater is constructed with.
fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

fn temp_data_root(tag: &str) -> DataPaths {
    let root = std::env::temp_dir().join(format!(
        "lazarus-updater-{tag}-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    DataPaths::at(root)
}

const ARTIFACT_NAME: &str = "lazarus-hostd";

fn payload(version_tag: u8) -> Vec<u8> {
    // Distinct plausible daemon binaries per release.
    vec![version_tag; 4096 + version_tag as usize * 13]
}

fn sha_of(bytes: &[u8]) -> String {
    lazarus_cli::updater::download::sha256_hex(bytes)
}

fn manifest_for(key: &SigningKey, version: &str, bytes: &[u8]) -> Vec<u8> {
    let manifest = ReleaseManifest {
        schema_version: 1,
        release: Release {
            version: version.to_owned(),
            artifact: Artifact {
                file_name: ARTIFACT_NAME.to_owned(),
                size_bytes: bytes.len() as u64,
                sha256: sha_of(bytes),
            },
        },
    };
    sign_manifest(&manifest, key).expect("signing")
}

fn write_manifest(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write manifest");
    path
}

/// A minimal HTTP release directory: serves the signed manifest at
/// `/manifest.json`, its artifact everywhere else, honors
/// `Range: bytes=N-` with 206 responses, and records each request's Range
/// header so tests can prove resumption happened.
struct TestServer {
    url: String,
    requests: Arc<Mutex<Vec<Option<String>>>>,
}

impl TestServer {
    fn start(manifest: Vec<u8>, artifact: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                if serve_one(&mut stream, &manifest, &artifact, &recorded).is_err() {
                    continue;
                }
            }
        });
        Self {
            url: format!("http://{addr}"),
            requests,
        }
    }

    fn manifest_url(&self) -> String {
        format!("{}/manifest.json", self.url)
    }

    /// The Range headers seen so far; `None` where a request had none.
    fn ranges(&self) -> Vec<Option<String>> {
        self.requests.lock().expect("request log").clone()
    }
}

fn serve_one(
    stream: &mut std::net::TcpStream,
    manifest: &[u8],
    artifact: &[u8],
    recorded: &Mutex<Vec<Option<String>>>,
) -> std::io::Result<()> {
    let mut buffer = [0u8; 8192];
    let mut head_end = 0usize;
    loop {
        let read = stream.read(&mut buffer[head_end..])?;
        if read == 0 {
            break;
        }
        head_end += read;
        if buffer[..head_end]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let wants_manifest = head.starts_with("GET /manifest.json");
    let range = head
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("range:"))
        .map(|line| line.split_once(':').expect("header").1.trim().to_owned());
    recorded.lock().expect("request log").push(range.clone());

    let payload = if wants_manifest { manifest } else { artifact };
    let total = payload.len();
    let start = match &range {
        Some(value) => value
            .strip_prefix("bytes=")
            .and_then(|rest| rest.split('-').next())
            .and_then(|prefix| prefix.parse::<usize>().ok())
            .unwrap_or(0),
        None => 0,
    };
    let body = payload.get(start..).unwrap_or_default();
    let response = if start == 0 {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n")
    } else {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            total.saturating_sub(1),
            body.len()
        )
    };
    stream.write_all(response.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn installed_record(paths: &DataPaths) -> Option<lazarus_cli::updater::layout::InstallRecord> {
    InstallPaths::from_data_paths(paths)
        .installed_record()
        .expect("install record readable")
}

fn installed_binary(paths: &DataPaths) -> Option<PathBuf> {
    let candidate = paths.host.join("install").join(ARTIFACT_NAME);
    candidate.exists().then_some(candidate)
}

#[tokio::test]
async fn invalid_signature_is_rejected_before_any_installation_happens() {
    let paths = temp_data_root("bad-sig");
    let release_dir = paths.root.join("releases");
    std::fs::create_dir_all(&release_dir).expect("release dir");

    // Signed by an impostor key; the updater pins a different trust root.
    let impostor = key(0xEE);
    let bytes = payload(1);
    let manifest_path = write_manifest(
        &release_dir,
        "release.json",
        &manifest_for(&impostor, "0.1.0", &bytes),
    );

    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let error = Updater::new(paths.clone(), trust_root)
        .apply(manifest_path.to_str().expect("utf8"), false)
        .await
        .expect_err("impostor manifest refused");
    assert!(
        error.to_string().contains("signature"),
        "rejection must name the signature failure: {error}"
    );
    assert!(
        installed_record(&paths).is_none(),
        "nothing may be installed from a bad signature"
    );
    std::fs::remove_dir_all(&paths.root).ok();
}

#[tokio::test]
async fn checksum_mismatch_is_rejected_and_promotes_nothing() {
    let paths = temp_data_root("bad-sha");
    let release_dir = paths.root.join("releases");
    std::fs::create_dir_all(&release_dir).expect("release dir");

    // The manifest honestly describes payload(1); the server serves
    // same-sized but different bytes, as a corrupted or hostile mirror
    // would, so the SHA-256 gate itself must catch it.
    let honest_bytes = payload(1);
    let mut corrupt_bytes = honest_bytes.clone();
    corrupt_bytes[0] ^= 0xFF;
    assert_eq!(corrupt_bytes.len(), honest_bytes.len());
    let manifest_bytes = manifest_for(&key(0x42), "0.1.0", &honest_bytes);
    let server = TestServer::start(manifest_bytes, corrupt_bytes);

    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let error = Updater::new(paths.clone(), trust_root)
        .apply(server.manifest_url().as_str(), false)
        .await
        .expect_err("checksum mismatch refused");
    let message = error.to_string();
    assert!(
        message.contains("checksum mismatch"),
        "rejection must name the checksum failure: {message}"
    );
    assert!(installed_record(&paths).is_none());
    assert!(
        !paths
            .host
            .join("install-staging")
            .join(ARTIFACT_NAME)
            .exists(),
        "a failed download must never reach staging"
    );
    std::fs::remove_dir_all(&paths.root).ok();
}

#[tokio::test]
async fn interrupted_download_resumes_from_the_persisted_partial() {
    let paths = temp_data_root("resume");
    let bytes = payload(2);
    let release_dir = paths.root.join("releases");
    std::fs::create_dir_all(&release_dir).expect("release dir");
    let manifest_bytes = manifest_for(&key(0x42), "0.2.0", &bytes);
    let server = TestServer::start(manifest_bytes.clone(), bytes.clone());

    // Simulate a previous interrupted attempt: half the artifact plus the
    // sidecar describing exactly that partial state.
    let partial_len = bytes.len() / 2;
    let cache = paths.host.join("download-cache");
    std::fs::create_dir_all(&cache).expect("cache dir");
    std::fs::write(
        cache.join(format!("{ARTIFACT_NAME}.part")),
        &bytes[..partial_len],
    )
    .expect("partial bytes");
    let artifact_url = format!("{}/{}", server.url, ARTIFACT_NAME);
    std::fs::write(
        cache.join(format!("{ARTIFACT_NAME}.part.json")),
        serde_json::json!({
            "url": artifact_url,
            "expectedSha256": sha_of(&bytes),
            "expectedSize": bytes.len(),
        })
        .to_string(),
    )
    .expect("partial meta");

    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let outcome = Updater::new(paths.clone(), trust_root)
        .apply(server.manifest_url().as_str(), false)
        .await
        .expect("resumed install succeeds");

    assert_eq!(
        outcome,
        ApplyOutcome::Installed {
            version: "0.2.0".to_owned(),
        }
    );
    let ranges = server.ranges();
    assert!(
        ranges
            .iter()
            .any(|range| range.as_deref() == Some(&format!("bytes={partial_len}-"))),
        "the downloader must have asked the server to resume from the partial offset: {ranges:?}"
    );
    let staged = installed_binary(&paths).expect("promoted binary");
    assert_eq!(
        std::fs::read(staged).expect("read"),
        bytes,
        "exact artifact bytes promoted"
    );
    assert!(
        !cache.join(format!("{ARTIFACT_NAME}.part")).exists(),
        "completed downloads leave no partial behind"
    );
    std::fs::remove_dir_all(&paths.root).ok();
}

#[tokio::test]
async fn update_promotes_atomically_then_rollback_restores_the_retained_install() {
    let paths = temp_data_root("update-rollback");
    let old_bytes = payload(1);
    let new_bytes = payload(3);
    let release_dir = paths.root.join("releases");
    std::fs::create_dir_all(&release_dir).expect("release dir");
    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let updater = Updater::new(paths.clone(), trust_root);

    // First install from a local manifest (offline bootstrap): the
    // artifact sits beside the manifest on disk.
    let v1 = write_manifest(
        &release_dir,
        "v1.json",
        &manifest_for(&key(0x42), "0.1.0", &old_bytes),
    );
    std::fs::write(release_dir.join(ARTIFACT_NAME), &old_bytes).expect("offline artifact");
    assert_eq!(
        updater
            .apply(v1.to_str().expect("utf8"), false)
            .await
            .expect("first install"),
        ApplyOutcome::Installed {
            version: "0.1.0".to_owned(),
        }
    );

    // Update over HTTP from a release directory that serves both the
    // signed manifest and its artifact, like production releases do.
    let manifest_bytes = manifest_for(&key(0x42), "0.2.0", &new_bytes);
    let server = TestServer::start(manifest_bytes, new_bytes.clone());
    assert_eq!(
        updater
            .apply(server.manifest_url().as_str(), false)
            .await
            .expect("update applies"),
        ApplyOutcome::Updated {
            from_version: Some("0.1.0".to_owned()),
            version: "0.2.0".to_owned(),
        }
    );

    // Promotion was atomic: the new record and bytes are live, and the
    // old install sits intact in the rollback slot.
    let live = installed_record(&paths).expect("live record");
    assert_eq!(live.version, "0.2.0");
    let staged = installed_binary(&paths).expect("binary");
    assert_eq!(std::fs::read(staged).expect("read"), new_bytes);
    let retained = InstallPaths::from_data_paths(&paths)
        .retained_record()
        .expect("retained readable");
    assert_eq!(
        retained.map(|record| record.version),
        Some("0.1.0".to_owned()),
        "the replaced installation is retained for explicit rollback"
    );

    // Explicit rollback restores the retained release byte-for-byte.
    let outcome = updater.rollback().expect("rollback");
    assert_eq!(outcome.restored_version, "0.1.0");
    let live = installed_record(&paths).expect("live record after rollback");
    assert_eq!(live.version, "0.1.0");
    let restored = installed_binary(&paths).expect("restored binary");
    assert_eq!(std::fs::read(restored).expect("read"), old_bytes);
    assert!(
        !paths.host.join("install.prev").exists(),
        "rollback consumes the retained copy"
    );
    std::fs::remove_dir_all(&paths.root).ok();
}

#[tokio::test]
async fn ensure_is_idempotent_when_the_installed_release_matches() {
    let paths = temp_data_root("ensure");
    let bytes = payload(5);
    let release_dir = paths.root.join("releases");
    std::fs::create_dir_all(&release_dir).expect("release dir");
    let manifest_path = write_manifest(
        &release_dir,
        "release.json",
        &manifest_for(&key(0x42), "9.9.9", &bytes),
    );
    std::fs::write(release_dir.join(ARTIFACT_NAME), &bytes).expect("offline artifact");
    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let updater = Updater::new(paths.clone(), trust_root);

    updater
        .apply(manifest_path.to_str().expect("utf8"), false)
        .await
        .expect("bootstrap install");

    // A second ensure recognizes the current release without touching it.
    assert_eq!(
        updater
            .apply(manifest_path.to_str().expect("utf8"), false)
            .await
            .expect("second ensure"),
        ApplyOutcome::AlreadyCurrent {
            version: "9.9.9".to_owned(),
        }
    );
    std::fs::remove_dir_all(&paths.root).ok();
}

#[tokio::test]
async fn promotion_is_refused_while_a_host_answers_on_the_recorded_address() {
    let paths = temp_data_root("in-use");
    let bytes = payload(6);
    let release_dir = paths.root.join("releases");
    std::fs::create_dir_all(&release_dir).expect("release dir");
    let manifest_path = write_manifest(
        &release_dir,
        "release.json",
        &manifest_for(&key(0x42), "0.3.0", &bytes),
    );

    // A live endpoint standing in for a running Host.
    let server = TestServer::start(vec![0u8; 16], vec![0u8; 16]);
    lazarus_cli::host::discovery::store_pid(
        &paths,
        &lazarus_cli::host::discovery::PidRecord {
            pid: 999_999,
            addr: server.url.clone(),
            version: "0.0.0".to_owned(),
        },
    )
    .expect("pid record");

    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let error = Updater::new(paths.clone(), trust_root)
        .apply(manifest_path.to_str().expect("utf8"), false)
        .await
        .expect_err("in-use promotion refused");
    assert!(
        error.to_string().contains("stop it"),
        "refusal must tell the user how to proceed: {error}"
    );
    assert!(installed_record(&paths).is_none());
    std::fs::remove_dir_all(&paths.root).ok();
}

#[tokio::test]
async fn rollback_without_a_retained_install_fails_clearly() {
    let paths = temp_data_root("no-rollback");
    let trust_root: VerifyingKey = key(0x42).verifying_key();
    let error = Updater::new(paths.clone(), trust_root)
        .rollback()
        .expect_err("nothing retained");
    assert!(
        error.to_string().contains("no retained installation"),
        "{error}"
    );
    std::fs::remove_dir_all(&paths.root).ok();
}
