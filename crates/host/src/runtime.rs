//! Local data-root and daemon lifecycle primitives.

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fs2::FileExt;
use serde::Serialize;

pub const DATA_DIR_ENV: &str = "LAZARUS_DATA_DIR";

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
    pub host: PathBuf,
    pub state: PathBuf,
    pub logs: PathBuf,
}

impl DataPaths {
    pub fn resolve() -> io::Result<Self> {
        let root = match std::env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
            Some(root) => PathBuf::from(root),
            None => home_dir()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable")
                })?
                .join(".lazarus"),
        };
        Ok(Self::at(root))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            host: root.join("host"),
            state: root.join("state"),
            logs: root.join("logs"),
            root,
        }
    }

    pub fn prepare(&self) -> io::Result<()> {
        fs::create_dir_all(&self.host)?;
        fs::create_dir_all(&self.state)?;
        fs::create_dir_all(&self.logs)
    }

    pub fn database(&self) -> PathBuf {
        self.state.join("lazarus.sqlite3")
    }

    fn lock_file(&self) -> PathBuf {
        self.host.join("host.lock")
    }

    fn crash_marker(&self) -> PathBuf {
        self.host.join("running.json")
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

#[derive(Debug)]
pub enum RuntimeError {
    AlreadyRunning(PathBuf),
    Io(io::Error),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning(path) => write!(
                f,
                "another lazarus-hostd instance already owns {}",
                path.display()
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AlreadyRunning(_) => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for RuntimeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// An OS-backed exclusive lock. The file may survive a crash, but its lock
/// does not, so stale files never prevent recovery.
pub struct InstanceLock {
    file: File,
    path: PathBuf,
    registry_key: PathBuf,
}

fn process_locks() -> &'static Mutex<HashSet<PathBuf>> {
    static LOCKS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
}

impl InstanceLock {
    pub fn acquire(paths: &DataPaths) -> Result<Self, RuntimeError> {
        paths.prepare()?;
        let path = paths.lock_file();
        let registry_key = fs::canonicalize(&paths.host)?.join("host.lock");
        let mut process_locks = process_locks()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if process_locks.contains(&registry_key) {
            return Err(RuntimeError::AlreadyRunning(path));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        FileExt::try_lock_exclusive(&file).map_err(|error| {
            let contended = fs2::lock_contended_error();
            if error.raw_os_error() == contended.raw_os_error() || error.kind() == contended.kind()
            {
                RuntimeError::AlreadyRunning(path.clone())
            } else {
                RuntimeError::Io(error)
            }
        })?;
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
        process_locks.insert(registry_key.clone());
        Ok(Self {
            file,
            path,
            registry_key,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
        process_locks()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.registry_key);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunningMarker<'a> {
    pid: u32,
    host_version: &'a str,
}

/// A marker whose continued presence means the prior process did not finish
/// graceful shutdown.
pub struct CrashMarker {
    path: PathBuf,
    previous_unclean_shutdown: bool,
}

impl CrashMarker {
    pub fn begin(paths: &DataPaths, host_version: &str) -> io::Result<Self> {
        paths.prepare()?;
        let path = paths.crash_marker();
        let previous_unclean_shutdown = path.exists();
        let mut file = File::create(&path)?;
        serde_json::to_writer(
            &mut file,
            &RunningMarker {
                pid: std::process::id(),
                host_version,
            },
        )?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(Self {
            path,
            previous_unclean_shutdown,
        })
    }

    pub fn previous_unclean_shutdown(&self) -> bool {
        self.previous_unclean_shutdown
    }

    pub fn mark_clean(self) -> io::Result<()> {
        fs::remove_file(self.path)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn temp_paths(tag: &str) -> DataPaths {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        DataPaths::at(std::env::temp_dir().join(format!(
            "lazarus-hostd-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    #[test]
    fn one_lock_owner_per_data_directory() {
        let paths = temp_paths("lock");
        let first = InstanceLock::acquire(&paths).expect("first instance owns the lock");
        assert!(matches!(
            InstanceLock::acquire(&paths),
            Err(RuntimeError::AlreadyRunning(_))
        ));
        drop(first);
        InstanceLock::acquire(&paths).expect("stale lock file is safe after owner exits");
        fs::remove_dir_all(&paths.root).expect("cleanup");
    }

    #[test]
    fn marker_distinguishes_clean_and_unclean_shutdown() {
        let paths = temp_paths("marker");
        let marker = CrashMarker::begin(&paths, "0.1.0").expect("first start");
        assert!(!marker.previous_unclean_shutdown());
        drop(marker);

        let recovered = CrashMarker::begin(&paths, "0.1.0").expect("recovery start");
        assert!(recovered.previous_unclean_shutdown());
        recovered.mark_clean().expect("clean stop removes marker");

        let clean_restart = CrashMarker::begin(&paths, "0.1.0").expect("clean restart");
        assert!(!clean_restart.previous_unclean_shutdown());
        clean_restart.mark_clean().expect("clean stop");
        fs::remove_dir_all(&paths.root).expect("cleanup");
    }
}
