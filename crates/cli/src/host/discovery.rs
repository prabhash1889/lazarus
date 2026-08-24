//! Discovery records under the data root that let the CLI (and anything it
//! launches, such as the Desktop) find the Host without guessing: the
//! per-install local token and the running-instance record.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use lazarus_hostd::runtime::DataPaths;
use serde::{Deserialize, Serialize};

/// The default loopback listen address used when nothing else names one.
pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";

fn token_path(paths: &DataPaths) -> PathBuf {
    paths.root.join("auth").join("local-token")
}

fn pid_path(paths: &DataPaths) -> PathBuf {
    paths.host.join("pid.json")
}

/// `<data>/logs/host.log`, the daemon's appended stdout/stderr capture.
pub fn log_path(paths: &DataPaths) -> PathBuf {
    paths.logs.join("host.log")
}

/// What `host start` records so later commands can find the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PidRecord {
    pub pid: u32,
    /// The HTTP base URL the daemon answers on.
    pub addr: String,
    pub version: String,
}

pub fn load_pid(paths: &DataPaths) -> Result<Option<PidRecord>> {
    let path = pid_path(paths);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).map(Some).with_context(|| {
        format!(
            "{} is corrupt; run `lazarus host stop` to clean up",
            path.display()
        )
    })
}

pub fn store_pid(paths: &DataPaths, record: &PidRecord) -> Result<()> {
    paths.prepare()?;
    let path = pid_path(paths);
    let mut file = File::create(&path).with_context(|| format!("writing {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, record)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all().ok();
    Ok(())
}

pub fn clear_pid(paths: &DataPaths) -> Result<()> {
    match fs::remove_file(pid_path(paths)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("removing the recorded instance"),
    }
}

pub fn read_token(paths: &DataPaths) -> Result<Option<String>> {
    let path = token_path(paths);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    match raw.trim() {
        "" => bail!(
            "{} exists but is empty; delete it to re-provision",
            path.display()
        ),
        token => Ok(Some(token.to_owned())),
    }
}

/// Loads the per-install local token, generating and persisting a fresh
/// high-entropy secret on first use. The value never appears in errors or
/// logs; on Unix the file is user-only (0600), and on Windows the data root
/// lives inside the user profile whose default ACLs already restrict it to
/// the current user and administrators.
pub fn load_or_create_token(paths: &DataPaths) -> Result<String> {
    if let Some(token) = read_token(paths)? {
        return Ok(token);
    }
    let token = generate_token()?;
    let path = token_path(paths);
    fs::create_dir_all(path.parent().expect("token path has a parent"))?;
    let mut file =
        File::create(&path).with_context(|| format!("provisioning {}", path.display()))?;
    file.write_all(token.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .with_context(|| format!("provisioning {}", path.display()))?;
    restrict_to_user(&path)?;
    Ok(token)
}

#[cfg(unix)]
fn restrict_to_user(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_to_user(_path: &Path) -> Result<()> {
    Ok(())
}

fn generate_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("generating the local token failed: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_paths(tag: &str) -> DataPaths {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        DataPaths::at(std::env::temp_dir().join(format!(
            "lazarus-cli-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    #[test]
    fn token_is_provisioned_once_and_reused() {
        let paths = temp_paths("token");
        let first = load_or_create_token(&paths).expect("first provisioning");
        assert!(!first.is_empty(), "the provisioned token is non-empty");
        let second = load_or_create_token(&paths).expect("reload");
        assert_eq!(first, second, "the token must be stable per install");

        // Distinct data roots get independent secrets.
        let other = load_or_create_token(&temp_paths("token")).expect("other root");
        assert_ne!(first, other, "tokens are generated per install");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(token_path(&paths))
                .expect("token metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "token file must be owner-only: {mode:o}");
        }
        fs::remove_dir_all(paths.root).expect("cleanup");
    }

    #[test]
    fn empty_token_file_is_an_error_not_a_silent_regeneration() {
        let paths = temp_paths("empty-token");
        paths.prepare().expect("prepare");
        fs::create_dir_all(token_path(&paths).parent().unwrap()).expect("auth dir");
        fs::write(token_path(&paths), "   \n").expect("blank token");
        let error = read_token(&paths).expect_err("blank token refused");
        assert!(error.to_string().contains("empty"), "{error}");
        fs::remove_dir_all(paths.root).expect("cleanup");
    }

    #[test]
    fn instance_record_round_trips_and_clears() {
        let paths = temp_paths("pid");
        assert_eq!(load_pid(&paths).expect("absent record"), None);

        let record = PidRecord {
            pid: 4242,
            addr: "http://127.0.0.1:50051".to_owned(),
            version: "0.1.0".to_owned(),
        };
        store_pid(&paths, &record).expect("store");
        assert_eq!(load_pid(&paths).expect("loaded"), Some(record.clone()));

        clear_pid(&paths).expect("clear");
        assert_eq!(load_pid(&paths).expect("cleared"), None);
        clear_pid(&paths).expect("clearing an absent record stays fine");
        fs::remove_dir_all(paths.root).expect("cleanup");
    }

    #[test]
    fn corrupt_instance_record_names_the_file_instead_of_guessing() {
        let paths = temp_paths("pid-corrupt");
        paths.prepare().expect("prepare");
        fs::write(pid_path(&paths), "{not json").expect("corrupt record");
        let error = load_pid(&paths).expect_err("corrupt record refused");
        assert!(
            error.to_string().contains("corrupt") && error.to_string().contains("stop"),
            "{error}"
        );
        fs::remove_dir_all(paths.root).expect("cleanup");
    }
}
