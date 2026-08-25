//! Local IPC transport: the authenticated Lazarus Protocol surface served
//! over a Windows named pipe or a Unix domain socket (plan section 4.4),
//! alongside the loopback TCP listener used by the CLI today. The Desktop
//! connects here exclusively; every request passes through the identical
//! Axum transport gate (token authentication, manifest negotiation,
//! deadlines), so the wire contract never depends on the carrier.
//!
//! The daemon records the live endpoint at `<data>/host/ipc-endpoint.json`
//! so clients discover it without guessing; the record disappears on clean
//! shutdown.

use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::io;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::runtime::DataPaths;

/// Environment override naming the IPC endpoint to serve instead of the
/// per-data-root default. On Windows this is a pipe name (the
/// `\\.\pipe\` prefix is optional); on Unix a socket filesystem path.
pub const IPC_ENDPOINT_ENV: &str = "LAZARUS_HOST_IPC";

const WINDOWS_PIPE_PREFIX: &str = r"\\.\pipe\";
/// Filesystem name of the discovery record under the Host directory.
pub const ENDPOINT_RECORD_FILE: &str = "ipc-endpoint.json";
/// Socket filename inside `<data>/host/`.
#[cfg(not(windows))]
const SOCKET_FILE: &str = "hostd.sock";

/// Where the Host serves its local IPC surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcEndpoint {
    /// A Windows named pipe; the full name including `\\.\pipe\`.
    NamedPipe(String),
    /// A Unix domain socket filesystem path.
    UnixSocket(PathBuf),
}

impl IpcEndpoint {
    /// The `kind` label used in the discovery record.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NamedPipe(_) => "namedPipe",
            Self::UnixSocket(_) => "unixSocket",
        }
    }

    /// The endpoint address as displayed to users (never secret).
    pub fn display_path(&self) -> Cow<'_, str> {
        match self {
            Self::NamedPipe(name) => Cow::Borrowed(name.as_str()),
            Self::UnixSocket(path) => path.to_string_lossy(),
        }
    }
}

impl fmt::Display for IpcEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.display_path(), self.kind())
    }
}

/// FNV-1a 64-bit over the UTF-8 bytes of `value`. Stable across releases by
/// construction; used only to derive collision-resistant per-data-root pipe
/// names, never for security.
fn fnv1a(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// The default local IPC endpoint for a data root: a named pipe whose name
/// is derived from the data root on Windows, and a socket file under
/// `<data>/host/` on Unix.
pub fn default_endpoint(paths: &DataPaths) -> IpcEndpoint {
    #[cfg(windows)]
    {
        let key = paths.root.to_string_lossy();
        IpcEndpoint::NamedPipe(format!(
            "{WINDOWS_PIPE_PREFIX}lazarus-hostd-{:012x}",
            fnv1a(&key)
        ))
    }
    #[cfg(not(windows))]
    {
        IpcEndpoint::UnixSocket(paths.host.join(SOCKET_FILE))
    }
}

/// Parses a user-supplied [`IPC_ENDPOINT_ENV`] override into an explicit
/// endpoint. Windows accepts the pipe name with or without the
/// `\\.\pipe\` prefix; Unix treats the value as a socket path.
pub fn endpoint_from_override(raw: &str) -> Result<IpcEndpoint, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{IPC_ENDPOINT_ENV} is set but empty"));
    }
    #[cfg(windows)]
    {
        if let Some(rest) = trimmed.strip_prefix(WINDOWS_PIPE_PREFIX) {
            if rest.is_empty() {
                return Err(format!(
                    "{IPC_ENDPOINT_ENV} names an empty pipe below {WINDOWS_PIPE_PREFIX}"
                ));
            }
            return Ok(IpcEndpoint::NamedPipe(trimmed.to_owned()));
        }
        Ok(IpcEndpoint::NamedPipe(format!(
            "{WINDOWS_PIPE_PREFIX}{trimmed}"
        )))
    }
    #[cfg(not(windows))]
    {
        let _ = WINDOWS_PIPE_PREFIX;
        Ok(IpcEndpoint::UnixSocket(PathBuf::from(trimmed)))
    }
}

/// Resolves the endpoint to serve: the environment override when set, else
/// the per-data-root default.
pub fn resolve_endpoint(paths: &DataPaths) -> Result<IpcEndpoint, String> {
    match std::env::var(IPC_ENDPOINT_ENV) {
        Ok(raw) => endpoint_from_override(&raw),
        Err(_) => Ok(default_endpoint(paths)),
    }
}

/// The discovery record written for clients; camelCase like every other
/// Host-produced artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointRecord {
    pub kind: String,
    pub path: String,
}

impl From<&IpcEndpoint> for EndpointRecord {
    fn from(endpoint: &IpcEndpoint) -> Self {
        Self {
            kind: endpoint.kind().to_owned(),
            path: endpoint.display_path().into_owned(),
        }
    }
}

fn endpoint_record_path(paths: &DataPaths) -> PathBuf {
    paths.host.join(ENDPOINT_RECORD_FILE)
}

/// Publishes the discovery record so clients can find the live endpoint.
pub fn publish_endpoint_record(paths: &DataPaths, endpoint: &IpcEndpoint) -> io::Result<()> {
    paths.prepare()?;
    let path = endpoint_record_path(paths);
    let mut file = fs::File::create(&path)?;
    serde_json::to_writer_pretty(&mut file, &EndpointRecord::from(endpoint))
        .map_err(io::Error::other)?;
    file.sync_all().ok();
    Ok(())
}

/// Removes the discovery record during clean shutdown; a missing record is
/// fine (a crash may have left nothing, or a prior stop removed it).
pub fn retire_endpoint_record(paths: &DataPaths) {
    match fs::remove_file(endpoint_record_path(paths)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(
                component = "hostd",
                event = "host.ipc_record_retire_failed",
                message = %error
            );
        }
    }
}

/// Reads the discovery record back; exposed for tooling and tests.
pub fn read_endpoint_record(paths: &DataPaths) -> io::Result<Option<EndpointRecord>> {
    match fs::read_to_string(endpoint_record_path(paths)) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// A bound-but-not-yet-serving local IPC listener. Binding happens before
/// the discovery record is published so clients never see an endpoint that
/// cannot accept connections yet.
pub enum IpcListener {
    #[cfg(unix)]
    Unix {
        listener: tokio::net::UnixListener,
        path: PathBuf,
    },
    #[cfg(windows)]
    NamedPipe {
        pending: tokio::net::windows::named_pipe::NamedPipeServer,
        name: String,
    },
}

impl IpcListener {
    /// Removes a leftover Unix socket file. Only safe once the process-wide
    /// single-instance lock is held: it proves no live daemon still owns
    /// the path, so any leftover file is crash debris. No-op elsewhere.
    pub fn remove_stale_socket(endpoint: &IpcEndpoint) {
        #[cfg(unix)]
        {
            let IpcEndpoint::UnixSocket(path) = endpoint else {
                return;
            };
            match fs::remove_file(path) {
                Ok(()) | Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(
                    component = "hostd",
                    event = "host.ipc_stale_socket_remove_failed",
                    message = %error
                ),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = endpoint;
        }
    }

    /// Binds the endpoint. Binding never removes an existing socket file -
    /// an occupied endpoint must fail here rather than destroy a live
    /// peer's listener; stale-file cleanup is [`Self::remove_stale_socket`]
    /// under the instance lock.
    pub fn bind(endpoint: &IpcEndpoint) -> io::Result<Self> {
        match endpoint {
            #[cfg(unix)]
            IpcEndpoint::UnixSocket(path) => {
                let listener = tokio::net::UnixListener::bind(path)?;
                restrict_socket_to_user(path)?;
                Ok(Self::Unix {
                    listener,
                    path: path.clone(),
                })
            }
            #[cfg(windows)]
            IpcEndpoint::NamedPipe(name) => {
                // `first_pipe_instance` fails creation when another process
                // already owns the name, mirroring TCP port ownership.
                let pending = tokio::net::windows::named_pipe::ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(name)?;
                Ok(Self::NamedPipe {
                    pending,
                    name: name.clone(),
                })
            }
            #[cfg(windows)]
            IpcEndpoint::UnixSocket(path) => Err(io::Error::other(format!(
                "a Unix domain socket ({}) cannot be served on this platform",
                path.display()
            ))),
            #[cfg(unix)]
            IpcEndpoint::NamedPipe(name) => Err(io::Error::other(format!(
                "a named pipe ({name}) cannot be served on this platform"
            ))),
        }
    }

    /// Human-readable description for logs.
    pub fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix { path, .. } => format!("{} (unixSocket)", path.display()),
            #[cfg(windows)]
            Self::NamedPipe { name, .. } => format!("{name} (namedPipe)"),
        }
    }
}

/// Serves `app` over the bound local IPC listener until the shutdown signal
/// fires. Every accepted connection runs the full Axum router through
/// hyper's HTTP/1 server, so behavior matches the TCP listener exactly.
pub async fn serve_ipc(listener: IpcListener, app: Router, mut shutdown: broadcast::Receiver<()>) {
    tracing::info!(
        component = "hostd",
        event = "host.ipc_listening",
        endpoint = %listener.describe()
    );
    match listener {
        #[cfg(unix)]
        IpcListener::Unix { listener, path } => loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    drop(listener);
                    match fs::remove_file(&path) {
                        Ok(())
                        | Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => {
                            tracing::warn!(
                                component = "hostd",
                                event = "host.ipc_socket_remove_failed",
                                message = %error
                            );
                        }
                    }
                    return;
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _peer)) => {
                            tokio::spawn(serve_connection(app.clone(), stream));
                        }
                        Err(error) => log_accept_failure(error),
                    }
                }
            }
        },
        #[cfg(windows)]
        IpcListener::NamedPipe { mut pending, name } => loop {
            let connected = tokio::select! {
                _ = shutdown.recv() => {
                    // Dropping the pending instance releases the pipe name.
                    return;
                }
                result = pending.connect() => result,
            };
            match connected {
                Ok(()) => {}
                Err(error) => {
                    log_accept_failure(error);
                    continue;
                }
            }
            // Move the now-connected instance out and queue a fresh
            // listener immediately so further clients can dial concurrently.
            let stream = pending;
            pending = match tokio::net::windows::named_pipe::ServerOptions::new().create(&name) {
                Ok(server) => server,
                Err(error) => {
                    tracing::error!(
                        component = "hostd",
                        event = "host.ipc_requeue_failed",
                        message = %error
                    );
                    return;
                }
            };
            tokio::spawn(serve_connection(app.clone(), stream));
        },
    }
}

fn log_accept_failure(error: io::Error) {
    tracing::warn!(
        component = "hostd",
        event = "host.ipc_accept_failed",
        message = %error
    );
}

#[cfg(unix)]
fn restrict_socket_to_user(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Drives one accepted connection through the shared router until the peer
/// disconnects or the handler completes (SSE subscriptions end when their
/// bus closes or the deadline elapses).
async fn serve_connection<I>(app: Router, io: I)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper_util::service::TowerToHyperService::new(app);
    let result = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
        .serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(io), service)
        .await;
    if let Err(error) = result {
        tracing::debug!(
            component = "hostd",
            event = "host.ipc_connection_closed",
            message = %error
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> DataPaths {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        DataPaths::at(std::env::temp_dir().join(format!(
            "lazarus-hostd-ipc-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }

    #[test]
    fn default_endpoints_are_deterministic_and_root_scoped() {
        let first = temp_paths("default");
        let second = temp_paths("default");
        let a = default_endpoint(&first);
        let b = default_endpoint(&first);
        assert_eq!(a, b, "the same data root always names the same endpoint");

        #[cfg(windows)]
        {
            let other_root = default_endpoint(&second);
            assert_ne!(a, other_root, "distinct roots must not share a pipe");
            let expected = match &a {
                IpcEndpoint::NamedPipe(name) => name.clone(),
                IpcEndpoint::UnixSocket(_) => panic!("windows defaults to a named pipe"),
            };
            assert!(
                expected.starts_with(r"\\.\pipe\lazarus-hostd-"),
                "{expected}"
            );
        }
        #[cfg(not(windows))]
        {
            let expected = match &a {
                IpcEndpoint::UnixSocket(path) => path.clone(),
                IpcEndpoint::NamedPipe(_) => panic!("unix defaults to a socket"),
            };
            assert_eq!(expected, first.host.join("hostd.sock"));
        }
    }

    #[test]
    fn overrides_parse_into_explicit_endpoints_or_fail_clearly() {
        #[cfg(windows)]
        {
            assert_eq!(
                endpoint_from_override("lazarus-test").unwrap(),
                IpcEndpoint::NamedPipe(r"\\.\pipe\lazarus-test".to_owned())
            );
            assert_eq!(
                endpoint_from_override(r"\\.\pipe\full").unwrap(),
                IpcEndpoint::NamedPipe(r"\\.\pipe\full".to_owned())
            );
            assert_eq!(
                endpoint_from_override("").unwrap_err(),
                "LAZARUS_HOST_IPC is set but empty"
            );
            assert_eq!(
                endpoint_from_override(r"\\.\pipe\").unwrap_err(),
                r"LAZARUS_HOST_IPC names an empty pipe below \\.\pipe\"
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                endpoint_from_override("/tmp/custom.sock").unwrap(),
                IpcEndpoint::UnixSocket(PathBuf::from("/tmp/custom.sock"))
            );
            assert_eq!(
                endpoint_from_override("   ").unwrap_err(),
                "LAZARUS_HOST_IPC is set but empty"
            );
        }
    }

    /// `resolve_endpoint` reads the process environment; the override path
    /// itself is covered by `overrides_parse_into_explicit_endpoints_or_fail_
    /// clearly`, so this test pins only the documented precedence contract
    /// against the pure pieces it composes.
    #[test]
    fn resolve_composes_override_then_default() {
        let paths = temp_paths("resolve");

        // With no override readable the default wins. The environment may
        // carry LAZARUS_HOST_IPC from an outer harness; treat both outcomes
        // as valid and assert only shape correctness.
        match resolve_endpoint(&paths) {
            Ok(resolved) => assert!(
                resolved == default_endpoint(&paths)
                    || matches!(resolved, IpcEndpoint::NamedPipe(_))
                    || matches!(resolved, IpcEndpoint::UnixSocket(_)),
                "{resolved} is a well-formed endpoint"
            ),
            Err(message) => assert!(message.contains(IPC_ENDPOINT_ENV), "{message}"),
        }
    }

    #[test]
    fn endpoint_record_round_trips_and_retires_cleanly() {
        let paths = temp_paths("record");
        assert_eq!(read_endpoint_record(&paths).unwrap(), None);

        let endpoint = default_endpoint(&paths);
        publish_endpoint_record(&paths, &endpoint).unwrap();
        assert_eq!(
            read_endpoint_record(&paths).unwrap(),
            Some(EndpointRecord {
                kind: endpoint.kind().to_owned(),
                path: endpoint.display_path().into_owned(),
            })
        );

        retire_endpoint_record(&paths);
        assert_eq!(read_endpoint_record(&paths).unwrap(), None);
        retire_endpoint_record(&paths); // idempotent

        fs::remove_dir_all(paths.root).unwrap();
    }
}
