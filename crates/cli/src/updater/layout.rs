//! On-disk install layout under `<data-root>/host` and the small JSON
//! records that make every state inspectable:
//!
//! ```text
//! host/
//!   lifecycle.lock          # the one cross-process mutation lock
//!   install/                # the live installation
//!     lazarus-install.json  # what is currently promoted
//!     <artifact files>
//!   install-staging/        # a fully verified candidate + staged.json
//!     lazarus-staged.json
//!   install.prev/           # the retained rename-aside for rollback
//!     lazarus-install.json
//!   download-cache/         # resumable partial downloads (.part + meta)
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use lazarus_hostd::runtime::DataPaths;

pub const RECORD_SCHEMA_VERSION: u32 = 1;

const INSTALL_DIR: &str = "install";
const STAGING_DIR: &str = "install-staging";
const ROLLBACK_DIR: &str = "install.prev";
const DOWNLOAD_CACHE_DIR: &str = "download-cache";
const INSTALL_RECORD_FILE: &str = "lazarus-install.json";
const STAGED_RECORD_FILE: &str = "lazarus-staged.json";
pub const LIFECYCLE_LOCK_FILE: &str = "lifecycle.lock";

/// Resolved paths for one data root's Host installation.
#[derive(Debug, Clone)]
pub struct InstallPaths {
    base: PathBuf,
}

impl InstallPaths {
    pub fn from_data_paths(paths: &DataPaths) -> Self {
        Self {
            base: paths.host.clone(),
        }
    }

    pub fn at(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    pub fn install_dir(&self) -> PathBuf {
        self.base.join(INSTALL_DIR)
    }

    /// Lives inside `install/` so the record always travels atomically with
    /// the binaries it describes.
    pub fn install_record_path(&self) -> PathBuf {
        self.install_dir().join(INSTALL_RECORD_FILE)
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.base.join(STAGING_DIR)
    }

    pub fn staged_record_path(&self) -> PathBuf {
        self.staging_dir().join(STAGED_RECORD_FILE)
    }

    pub fn rollback_dir(&self) -> PathBuf {
        self.base.join(ROLLBACK_DIR)
    }

    pub fn download_cache_dir(&self) -> PathBuf {
        self.base.join(DOWNLOAD_CACHE_DIR)
    }

    pub fn lifecycle_lock_path(&self) -> PathBuf {
        self.base.join(LIFECYCLE_LOCK_FILE)
    }

    pub fn installed_record(&self) -> Result<Option<InstallRecord>> {
        load_optional_json(&self.install_record_path())
    }

    /// The record inside the retained rename-aside copy, which keeps its
    /// original file name but sits directly under `install.prev/`.
    pub fn retained_record(&self) -> Result<Option<InstallRecord>> {
        load_optional_json(&self.rollback_dir().join(INSTALL_RECORD_FILE))
    }

    pub fn staged_record(&self) -> Result<Option<StagedRecord>> {
        load_optional_json(&self.staged_record_path())
    }
}

/// What is currently promoted. Written inside `install/` before promotion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    pub schema_version: u32,
    pub version: String,
    pub artifact_sha256: String,
    pub artifact_file_name: String,
    pub promoted_at_unix: u64,
}

/// A verified candidate waiting in staging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedRecord {
    pub schema_version: u32,
    pub version: String,
    pub artifact_sha256: String,
    pub artifact_file_name: String,
    pub staged_at_unix: u64,
}

impl From<&StagedRecord> for InstallRecord {
    fn from(staged: &StagedRecord) -> Self {
        Self {
            schema_version: RECORD_SCHEMA_VERSION,
            version: staged.version.clone(),
            artifact_sha256: staged.artifact_sha256.clone(),
            artifact_file_name: staged.artifact_file_name.clone(),
            promoted_at_unix: unix_now(),
        }
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn load_optional_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("{} is corrupt; delete it to recover", path.display()))
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(value)
        .with_context(|| format!("encoding {}", path.display()))?;
    fs::write(path, raw).with_context(|| format!("writing {}", path.display()))
}
