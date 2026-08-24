//! The CLI-owned Host updater: install, update, and roll back the local
//! `lazarus-hostd` installation safely across interruptions.
//!
//! Pipeline per plan §10.1: verify a signed manifest, resume-download the
//! artifact into the download cache, checksum it, stage a complete
//! candidate under the cross-process lifecycle lock, then promote with a
//! rename-aside so the previous installation stays available for an
//! explicit `host rollback`.

pub mod download;
pub mod layout;
pub mod manifest;
pub mod trust;

use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use ed25519_dalek::VerifyingKey;
use fs2::FileExt;

use crate::host::discovery;
use lazarus_hostd::runtime::DataPaths;

use self::download::DownloadRequest;
use self::layout::{
    InstallPaths, InstallRecord, RECORD_SCHEMA_VERSION, StagedRecord, unix_now, write_json,
};
use self::manifest::{ReleaseManifest, verify_manifest};

/// How long an apply waits for a contended lifecycle lock before giving
/// up: long enough to outlive a concurrent short operation, short enough
/// that users are not left hanging.
const LOCK_WAIT: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Nothing was installed before this apply.
    Installed { version: String },
    /// An existing installation was replaced.
    Updated {
        from_version: Option<String>,
        version: String,
    },
    /// The requested release is already what is installed (`host ensure`
    /// and non-forced updates).
    AlreadyCurrent { version: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackOutcome {
    pub restored_version: String,
}

/// Where the manifest came from and its verified contents.
struct ResolvedManifest {
    manifest: ReleaseManifest,
    /// Directory of the manifest when fetched over HTTP; release artifacts
    /// are published next to their manifest.
    http_base: Option<String>,
    /// Directory of the manifest when read from disk; artifacts may sit
    /// next to it for offline installs.
    local_base: Option<PathBuf>,
}

pub struct Updater {
    data_paths: DataPaths,
    paths: InstallPaths,
    trust_root: VerifyingKey,
    http: reqwest::Client,
}

impl Updater {
    pub fn new(data_paths: DataPaths, trust_root: VerifyingKey) -> Self {
        Self {
            paths: InstallPaths::from_data_paths(&data_paths),
            data_paths,
            trust_root,
            http: reqwest::Client::builder()
                // Release artifacts can be tens of megabytes on slow links;
                // give transfers room without letting them hang forever.
                .timeout(Duration::from_secs(600))
                .connect_timeout(Duration::from_secs(15))
                .build()
                .expect("static client configuration"),
        }
    }

    pub fn paths(&self) -> &InstallPaths {
        &self.paths
    }

    /// What is currently installed, if anything.
    pub fn installed(&self) -> Result<Option<InstallRecord>> {
        self.paths.installed_record()
    }

    /// Installs or updates the Host from `source` (an https URL or a local
    /// path to the signed manifest). Idempotent: when the installed release
    /// already matches the manifest, nothing is touched unless `force`.
    pub async fn apply(&self, source: &str, force: bool) -> Result<ApplyOutcome> {
        let resolved = load_and_verify_manifest(&self.http, source, &self.trust_root).await?;
        let version = resolved.manifest.release.version.clone();

        let _guard = LifecycleLock::acquire(&self.paths)?;

        if !force && self.matches_installed(&resolved.manifest)? {
            return Ok(ApplyOutcome::AlreadyCurrent { version });
        }

        // Never pull the binary out from under a live daemon (plan §10.1
        // step 13: reject in-use promotions).
        self.refuse_while_host_running(&version).await?;

        let completed = self.download_artifact(&resolved).await?;
        let staged = self.stage_artifact(&resolved.manifest, &completed.path)?;
        let previous = self.promote(staged)?;

        Ok(match previous {
            Some(from) => ApplyOutcome::Updated {
                from_version: Some(from.version),
                version,
            },
            None => ApplyOutcome::Installed { version },
        })
    }

    /// Swaps the retained rename-aside installation back in. The current
    /// installation is only destroyed after the rollback copy is in place.
    pub fn rollback(&self) -> Result<RollbackOutcome> {
        let _guard = LifecycleLock::acquire(&self.paths)?;

        let rollback_dir = self.paths.rollback_dir();
        let retained = self.paths.retained_record()?.ok_or_else(|| {
            anyhow!(
                "no retained installation to roll back to ({} is absent or has no record)",
                rollback_dir.join("lazarus-install.json").display()
            )
        })?;

        let current_install = self.paths.install_dir();
        if !current_install.exists() {
            rename_dir(&rollback_dir, &current_install)?;
            return Ok(RollbackOutcome {
                restored_version: retained.version,
            });
        }

        let discard_dir = self.data_paths.host.join("install.discarded");
        fs::remove_dir_all(&discard_dir).ok();
        rename_dir(&current_install, &discard_dir)
            .with_context(|| format!("setting aside {}", current_install.display()))?;
        if let Err(error) = rename_dir(&rollback_dir, &current_install) {
            // Put the current install back; never leave the machine with
            // neither installation present.
            rename_dir(&discard_dir, &current_install).with_context(|| {
                format!(
                    "restoring {} after a failed rollback; the Host installation is intact but unchanged",
                    current_install.display()
                )
            })?;
            return Err(error).context("rolling back to the retained installation");
        }
        fs::remove_dir_all(&discard_dir).ok();

        Ok(RollbackOutcome {
            restored_version: retained.version,
        })
    }

    fn matches_installed(&self, manifest: &ReleaseManifest) -> Result<bool> {
        let Some(installed) = self.paths.installed_record()? else {
            return Ok(false);
        };
        Ok(installed.version == manifest.release.version
            && installed.artifact_sha256 == manifest.release.artifact.sha256.to_ascii_lowercase())
    }

    async fn refuse_while_host_running(&self, version: &str) -> Result<()> {
        let Some(record) = discovery::load_pid(&self.data_paths)? else {
            return Ok(());
        };
        let token = discovery::read_token(&self.data_paths)?.unwrap_or_default();
        if crate::host::start::probe_reachable(&record.addr, &token).await {
            bail!(
                "the Host is running at {}; stop it with `lazarus host stop` before installing version {version}",
                record.addr
            );
        }
        Ok(())
    }

    async fn download_artifact(
        &self,
        resolved: &ResolvedManifest,
    ) -> Result<download::CompletedDownload> {
        let artifact = &resolved.manifest.release.artifact;
        let destination = self.paths.download_cache_dir().join(&artifact.file_name);

        if let Some(base) = &resolved.local_base {
            // Offline path: the artifact sits beside its manifest on disk.
            // Copy through the same verify-then-publish contract as the
            // network path so both routes end in identical state.
            let source = base.join(&artifact.file_name);
            if !source.exists() {
                bail!(
                    "local release manifest does not name an existing artifact; expected {}",
                    source.display()
                );
            }
            let size = fs::metadata(&source)
                .with_context(|| format!("checking {}", source.display()))?
                .len();
            if size != artifact.size_bytes {
                bail!(
                    "artifact {} is {} bytes but the signed manifest says {}",
                    source.display(),
                    size,
                    artifact.size_bytes
                );
            }
            download::require_digest_match(
                &artifact.sha256,
                &download::hash_file(&source).context("hashing the local artifact")?,
            )?;
            fs::create_dir_all(destination.parent().expect("cache dir has a parent"))?;
            fs::copy(&source, &destination)
                .with_context(|| format!("caching {}", source.display()))?;
            return Ok(download::CompletedDownload {
                path: destination.clone(),
                sha256: download::normalize_digest(&artifact.sha256),
                resumed_from: 0,
            });
        }

        let url = match &resolved.http_base {
            Some(base) => format!("{base}/{}", artifact.file_name),
            None => bail!("manifest source is neither an HTTP URL nor a file path"),
        };
        let request = DownloadRequest {
            url,
            destination,
            expected_sha256: artifact.sha256.clone(),
            expected_size: artifact.size_bytes,
        };
        // Record intent before the first byte lands so any interruption
        // leaves resumable state behind rather than orphan bytes.
        download::persist_partial_meta(&request)?;
        let completed = download::download_resumable(&self.http, &request).await?;
        if completed.resumed_from > 0 {
            tracing::info!(
                resumed_from = completed.resumed_from,
                "resumed interrupted Host download"
            );
        }
        Ok(completed)
    }

    /// Copies the verified artifact into staging with its record. Staging
    /// is always rebuilt so stale candidates cannot survive.
    fn stage_artifact(
        &self,
        manifest: &ReleaseManifest,
        downloaded: &Path,
    ) -> Result<StagedRecord> {
        let staging = self.paths.staging_dir();
        fs::remove_dir_all(&staging).ok();
        fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;

        let target = staging.join(&manifest.release.artifact.file_name);
        fs::copy(downloaded, &target).with_context(|| {
            format!("staging {} into {}", downloaded.display(), target.display())
        })?;
        // Re-verify the staged copy: promotion must move exactly the bytes
        // the manifest checksummed, even if something raced the cache file.
        let staged_digest = download::hash_file(&target).context("hashing the staged artifact")?;
        download::require_digest_match(&manifest.release.artifact.sha256, &staged_digest)?;

        let record = StagedRecord {
            schema_version: RECORD_SCHEMA_VERSION,
            version: manifest.release.version.clone(),
            artifact_sha256: manifest.release.artifact.sha256.to_ascii_lowercase(),
            artifact_file_name: manifest.release.artifact.file_name.clone(),
            staged_at_unix: unix_now(),
        };
        write_json(&self.paths.staged_record_path(), &record)?;
        Ok(record)
    }

    /// Atomically swaps the staged candidate into `install/`, retaining
    /// the replaced installation as the rollback copy. Returns the record
    /// of what was replaced, if anything.
    fn promote(&self, staged: StagedRecord) -> Result<Option<InstallRecord>> {
        let install = self.paths.install_dir();
        let rollback = self.paths.rollback_dir();

        let previous = if install.exists() {
            let record = match self.paths.installed_record()? {
                Some(record) => record,
                None => bail!(
                    "{} exists without an install record; delete it to repair the layout",
                    install.display()
                ),
            };
            fs::remove_dir_all(&rollback).ok();
            rename_dir(&install, &rollback).with_context(|| {
                format!("retaining {} as {}", install.display(), rollback.display())
            })?;
            Some(record)
        } else {
            None
        };

        if let Err(error) = rename_dir(&self.paths.staging_dir(), &install) {
            if previous.is_some() {
                // Restore the old install; a failed promotion must never
                // leave the machine without a usable Host.
                rename_dir(&rollback, &install).with_context(|| {
                    format!(
                        "restoring {} after a failed promotion; the previous installation is intact but not updated",
                        install.display()
                    )
                })?;
            }
            return Err(error).context("promoting the staged installation");
        }

        // The install record was written inside staging before the swap;
        // stamp promotion time now that the swap succeeded, and drop the
        // staging bookkeeping so the live install carries only its own.
        let record = InstallRecord::from(&staged);
        write_json(&self.paths.install_record_path(), &record)?;
        fs::remove_file(install.join("lazarus-staged.json")).ok();
        Ok(previous)
    }
}

async fn load_and_verify_manifest(
    http: &reqwest::Client,
    source: &str,
    trust_root: &VerifyingKey,
) -> Result<ResolvedManifest> {
    let bytes = if is_http(source) {
        let response = http
            .get(source)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .with_context(|| format!("fetching release manifest from {source}"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("release manifest fetch returned HTTP {}", status.as_u16());
        }
        response
            .bytes()
            .await
            .context("reading the release manifest")?
            .to_vec()
    } else {
        fs::read(source).with_context(|| format!("reading release manifest {source}"))?
    };
    let manifest = verify_manifest(&bytes, trust_root)
        .map_err(|error| anyhow!("release manifest rejected: {error}"))?;
    let local_base = if is_http(source) {
        None
    } else {
        Some(
            PathBuf::from(source)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        )
    };
    Ok(ResolvedManifest {
        http_base: http_base_of(source),
        local_base,
        manifest,
    })
}

fn is_http(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// `https://releases.example/0.2/manifest.json` yields
/// `https://releases.example/0.2`; file paths yield `None`.
fn http_base_of(source: &str) -> Option<String> {
    if !is_http(source) {
        return None;
    }
    let trimmed = source.trim_end_matches('/');
    trimmed.rsplit_once('/').map(|(base, _)| base.to_owned())
}

/// Held for the duration of one mutation; dropping releases it.
struct LifecycleLock {
    _file: File,
}

impl LifecycleLock {
    /// Acquires the one cross-process mutation lock (plan §10.1 step 12):
    /// every install/update/rollback on this machine serializes through
    /// it, whether invoked by the CLI directly or via the Desktop.
    fn acquire(paths: &InstallPaths) -> Result<Self> {
        let lock_path = paths.lifecycle_lock_path();
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let file =
            File::create(&lock_path).with_context(|| format!("opening {}", lock_path.display()))?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { _file: file }),
                Err(_) if Instant::now() < deadline => std::thread::sleep(LOCK_POLL),
                Err(_) => bail!(
                    "another Host lifecycle operation holds {}; retry when it finishes",
                    lock_path.display()
                ),
            }
        }
    }
}

fn rename_dir(from: &Path, to: &PathBuf) -> Result<()> {
    fs::rename(from, to).with_context(|| format!("renaming {} to {}", from.display(), to.display()))
}
