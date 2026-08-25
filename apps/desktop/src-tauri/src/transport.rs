//! The Desktop's transport bridge to the local Host: unary protocol calls
//! and the `system.subscribeEvents` stream carried over the Host's local
//! named pipe / Unix domain socket endpoint (discovered from
//! `<data>/host/ipc-endpoint.json`, written by lazarus-hostd).
//!
//! Responsibilities kept deliberately thin: dial, attach the contract
//! headers (`Authorization` bearer token, this client's complete per-method
//! manifest, the caller deadline), enforce the shared budget as a hard
//! timeout, hand back typed errors as data - never hang. The local token is
//! resolved here so it never enters the webview. Resilience policy
//! (state machine, backoff, resubscription) lives in the TypeScript client;
//! Rust only opens what it is told to open.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use http_body_util::BodyExt;
use hyper::Request;
use hyper_util::rt::TokioIo;
use protocol_rs::auth::{self, LOCAL_TOKEN_ENV, bearer_header};
use protocol_rs::deadline::{self, CLIENT_TIMEOUT_GRACE_MS, DEFAULT_RPC_BUDGET_MS, Deadline};
use protocol_rs::generated_registry::wire;
use protocol_rs::manifest::{MANIFEST_METADATA_KEY, host_manifest_encoded};
use serde::{Deserialize, Serialize};
use tauri::Emitter;

/// The Host writes this record on every start; see
/// `crates/host/src/ipc.rs` (ENDPOINT_RECORD_FILE). Duplicated as a literal
/// because the desktop intentionally avoids depending on the whole daemon
/// crate.
const ENDPOINT_RECORD_FILE: &str = "ipc-endpoint.json";
/// Header naming the last outage a reconnecting subscriber already knows;
/// mirrors `LAST_OUTAGE_HEADER` in the daemon.
const LAST_OUTAGE_HEADER: &str = "x-lazarus-last-outage-id";

/// Tauri event carrying one decoded `system.subscribeEvents` frame payload.
pub const EVENT_FRAME_EVENT: &str = "lazarus://event-frame";
/// Tauri event emitted when the event stream ends; `reason` explains why.
pub const EVENTS_CLOSED_EVENT: &str = "lazarus://events-closed";

// ---------------------------------------------------------------------------
// Wire data types crossing the Tauri IPC boundary (camelCase for JS).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypedError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl TypedError {
    fn new(code: wire::ProtocolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.as_str().to_owned(),
            message: message.into(),
            retryable: code.is_retryable(),
        }
    }
}

/// The result of one unary call over the local IPC transport. Failures are
/// values, not rejections, so callers always get a renderable answer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcResponse {
    pub ok: bool,
    pub status: Option<u16>,
    /// The Host's advertised method manifest from a successful response.
    pub manifest: Option<String>,
    pub body: Option<String>,
    pub error: Option<TypedError>,
}

impl IpcResponse {
    fn success(status: u16, manifest: Option<String>, body: String) -> Self {
        Self {
            ok: true,
            status: Some(status),
            manifest,
            body: Some(body),
            error: None,
        }
    }

    fn failure(error: TypedError) -> Self {
        Self {
            ok: false,
            status: None,
            manifest: None,
            body: None,
            error: Some(error),
        }
    }

    fn rejected(status: u16, body: String, error: TypedError) -> Self {
        Self {
            ok: false,
            status: Some(status),
            manifest: None,
            body: Some(body),
            error: Some(error),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnaryArgs {
    pub request_id: u64,
    /// Protocol path served by the Host, e.g. `/system/info`.
    pub path: String,
    /// HTTP verb; defaults to GET.
    pub http_method: Option<String>,
    /// JSON-encoded request payload when the method takes a body.
    pub payload: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsOpenArgs {
    /// The last outage id this client already applied; the Host suppresses
    /// a redundant tombstone when it matches the current incarnation.
    pub last_outage_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Cancellation registry: request id -> cancel channel.
// ---------------------------------------------------------------------------

fn cancellations() -> &'static Mutex<HashMap<u64, tokio::sync::oneshot::Sender<()>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<()>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_cancel(request_id: u64) -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if let Ok(mut registry) = cancellations().lock() {
        // A duplicate id replaces the older sender, which drops it and makes
        // any stale wait fail fast rather than leak.
        registry.insert(request_id, tx);
    }
    rx
}

fn deregister_cancel(request_id: u64) {
    if let Ok(mut registry) = cancellations().lock() {
        registry.remove(&request_id);
    }
}

/// Aborts an in-flight [`host_ipc_request`] call. Returns whether a live
/// request was cancelled. The caller observes a typed `CANCELLED` error.
pub fn cancel_ipc_request(request_id: u64) -> bool {
    let sender = cancellations()
        .lock()
        .ok()
        .and_then(|mut registry| registry.remove(&request_id));
    match sender {
        Some(sender) => sender.send(()).is_ok(),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Endpoint + token discovery.
// ---------------------------------------------------------------------------

/// One discovered Host IPC endpoint plus where its token came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredHost {
    pub kind: String,
    pub path: String,
}

#[derive(Debug)]
pub enum DiscoveryError {
    MissingToken,
    EmptyTokenEnv,
    EmptyTokenFile,
    NoDataRoot,
    EndpointMissing { root: PathBuf },
    EndpointCorrupt(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToken => {
                write!(
                    f,
                    "no local token available; run `lazarus host start` first"
                )
            }
            Self::EmptyTokenEnv => write!(f, "{LOCAL_TOKEN_ENV} is set but empty"),
            Self::EmptyTokenFile => write!(
                f,
                "the per-install token file exists but is empty; run `lazarus host start` to re-provision"
            ),
            Self::NoDataRoot => write!(
                f,
                "cannot resolve the Lazarus data root (no LAZARUS_DATA_DIR or home directory)"
            ),
            Self::EndpointMissing { root } => write!(
                f,
                "the Host endpoint record ({ENDPOINT_RECORD_FILE}) was not found under {}; is lazarus-hostd running?",
                root.display()
            ),
            Self::EndpointCorrupt(detail) => {
                write!(f, "the Host endpoint record is unreadable: {detail}")
            }
        }
    }
}

/// The Lazarus data root: `LAZARUS_DATA_DIR` when set, else the user home.
fn data_root() -> Result<PathBuf, DiscoveryError> {
    if let Some(root) = std::env::var_os("LAZARUS_DATA_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    for key in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(home).join(".lazarus"));
        }
    }
    Err(DiscoveryError::NoDataRoot)
}

/// Pure token selection across the two provision sources; the value never
/// appears in error messages.
fn choose_token(from_env: Option<&str>, from_file: Option<&str>) -> Result<String, DiscoveryError> {
    match from_env.map(str::trim) {
        Some("") => Err(DiscoveryError::EmptyTokenEnv),
        Some(token) => Ok(token.to_owned()),
        None => match from_file.map(str::trim) {
            None => Err(DiscoveryError::MissingToken),
            Some("") => Err(DiscoveryError::EmptyTokenFile),
            Some(token) => Ok(token.to_owned()),
        },
    }
}

/// Resolves the local token exactly like the daemon/CLI do: environment
/// first, then the per-install file `<data>/auth/local-token`.
fn resolve_token(root: &std::path::Path) -> Result<String, DiscoveryError> {
    let from_env = std::env::var(LOCAL_TOKEN_ENV).ok();
    let from_file = std::fs::read_to_string(root.join("auth").join("local-token")).ok();
    choose_token(from_env.as_deref(), from_file.as_deref())
}

/// Reads the Host's endpoint record; `Ok(None)` means "not running here".
fn read_endpoint_record(root: &std::path::Path) -> Result<Option<DiscoveredHost>, DiscoveryError> {
    let path = root.join("host").join(ENDPOINT_RECORD_FILE);
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let parsed: EndpointRecordJson = serde_json::from_str(&raw)
                .map_err(|error| DiscoveryError::EndpointCorrupt(error.to_string()))?;
            Ok(Some(DiscoveredHost {
                kind: parsed.kind,
                path: parsed.path,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DiscoveryError::EndpointCorrupt(error.to_string())),
    }
}

#[derive(Debug, Deserialize)]
struct EndpointRecordJson {
    kind: String,
    path: String,
}

/// Discovers both halves needed to talk to the Host.
fn discover_host() -> Result<(DiscoveredHost, String), DiscoveryError> {
    let root = data_root()?;
    let token = resolve_token(&root)?;
    let endpoint = read_endpoint_record(&root)?.ok_or(DiscoveryError::EndpointMissing {
        root: root.join("host"),
    })?;
    Ok((endpoint, token))
}

// ---------------------------------------------------------------------------
// Dialing.
// ---------------------------------------------------------------------------

/// A fully-buffered request body for unary calls; every Lazarus payload is
/// small JSON.
fn request_body(payload: Option<&str>) -> http_body_util::Full<bytes::Bytes> {
    match payload {
        Some(payload) => {
            http_body_util::Full::new(bytes::Bytes::copy_from_slice(payload.as_bytes()))
        }
        None => http_body_util::Full::new(bytes::Bytes::new()),
    }
}

enum IpcStream {
    #[cfg(windows)]
    NamedPipe(tokio::net::windows::named_pipe::NamedPipeClient),
    #[cfg(not(windows))]
    UnixSocket(tokio::net::UnixStream),
}

impl tokio::io::AsyncRead for IpcStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut *self {
            #[cfg(windows)]
            IpcStream::NamedPipe(inner) => std::pin::Pin::new(inner).poll_read(cx, buf),
            #[cfg(not(windows))]
            IpcStream::UnixSocket(inner) => std::pin::Pin::new(inner).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for IpcStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match &mut *self {
            #[cfg(windows)]
            IpcStream::NamedPipe(inner) => std::pin::Pin::new(inner).poll_write(cx, buf),
            #[cfg(not(windows))]
            IpcStream::UnixSocket(inner) => std::pin::Pin::new(inner).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut *self {
            #[cfg(windows)]
            IpcStream::NamedPipe(inner) => std::pin::Pin::new(inner).poll_flush(cx),
            #[cfg(not(windows))]
            IpcStream::UnixSocket(inner) => std::pin::Pin::new(inner).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut *self {
            #[cfg(windows)]
            IpcStream::NamedPipe(inner) => std::pin::Pin::new(inner).poll_shutdown(cx),
            #[cfg(not(windows))]
            IpcStream::UnixSocket(inner) => std::pin::Pin::new(inner).poll_shutdown(cx),
        }
    }
}

fn dial(endpoint: &DiscoveredHost) -> io::Result<IpcStream> {
    match endpoint.kind.as_str() {
        #[cfg(windows)]
        "namedPipe" => {
            use tokio::net::windows::named_pipe::ClientOptions;

            Ok(IpcStream::NamedPipe(
                ClientOptions::new().open(&endpoint.path)?,
            ))
        }
        #[cfg(not(windows))]
        "unixSocket" => Ok(IpcStream::UnixSocket(tokio::net::UnixStream::connect(
            &endpoint.path,
        )?)),
        other => Err(io::Error::other(format!(
            "this platform cannot serve the Host endpoint kind {other:?}"
        ))),
    }
}

async fn dial_async(endpoint: &DiscoveredHost) -> io::Result<IpcStream> {
    dial(endpoint)
}

// ---------------------------------------------------------------------------
// Contract headers + unary execution.
// ---------------------------------------------------------------------------

/// The contract headers every unary request carries. Pure so tests can pin
/// the exact header set without touching the network.
fn contract_headers(token: &str) -> Result<Vec<(String, String)>, String> {
    let authorization = bearer_header(token);
    let stamp = Deadline::header_from_budget(deadline::unix_now_ms(), DEFAULT_RPC_BUDGET_MS);
    Ok(vec![
        (auth::AUTH_METADATA_KEY.to_owned(), authorization),
        (
            MANIFEST_METADATA_KEY.to_owned(),
            host_manifest_encoded().to_owned(),
        ),
        (deadline::DEADLINE_HEADER.to_owned(), stamp),
    ])
}

/// Executes one unary call end to end: dial, handshake, send with headers,
/// collect the bounded response. Transport failures map onto typed errors
/// at the command boundary.
async fn execute_unary(endpoint: &DiscoveredHost, args: &UnaryArgs, token: &str) -> IpcResponse {
    let headers = match contract_headers(token) {
        Ok(headers) => headers,
        Err(message) => {
            return IpcResponse::failure(TypedError::new(
                wire::ProtocolErrorCode::Internal,
                message,
            ));
        }
    };

    let stream = match dial_async(endpoint).await {
        Ok(stream) => stream,
        Err(error) => {
            return IpcResponse::failure(TypedError::new(
                wire::ProtocolErrorCode::Unavailable,
                format!("cannot reach the Host at {}: {error}", endpoint.path),
            ));
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(error) => {
            return IpcResponse::failure(TypedError::new(
                wire::ProtocolErrorCode::Unavailable,
                format!("the Host connection failed during setup: {error}"),
            ));
        }
    };
    tokio::spawn(async move {
        // Drives the connection to completion; dropping early cancels it.
        let _ = conn.await;
    });

    let mut builder = Request::builder()
        .method(args.http_method.as_deref().unwrap_or("GET"))
        .uri(args.path.as_str());
    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let request = match (&args.payload, builder) {
        (Some(payload), builder) => builder.body(request_body(Some(payload.as_str()))),
        (None, builder) => builder.body(request_body(None)),
    };
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            return IpcResponse::failure(TypedError::new(
                wire::ProtocolErrorCode::InvalidArgument,
                format!("request construction failed: {error}"),
            ));
        }
    };

    match sender.send_request(request).await {
        Ok(response) => {
            let status = response.status().as_u16();
            let manifest = response
                .headers()
                .get(MANIFEST_METADATA_KEY)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            match BodyExt::collect(response.into_body()).await {
                Ok(collected) => {
                    let body = String::from_utf8_lossy(&collected.to_bytes()).into_owned();
                    if (200..300).contains(&status) {
                        IpcResponse::success(status, manifest, body)
                    } else {
                        let error = decode_rejection(&status, &body);
                        IpcResponse::rejected(status, body, error)
                    }
                }
                Err(error) => IpcResponse::failure(TypedError::new(
                    wire::ProtocolErrorCode::Unavailable,
                    format!("reading the Host response failed: {error}"),
                )),
            }
        }
        Err(error) => IpcResponse::failure(TypedError::new(
            wire::ProtocolErrorCode::Unavailable,
            format!("the Host did not answer: {error}"),
        )),
    }
}

/// Maps a non-2xx response body onto the typed envelope when it conforms,
/// falling back to a generic INTERNAL error that cannot smuggle off-contract
/// payloads into callers.
fn decode_rejection(status: &u16, body: &str) -> TypedError {
    let parsed = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| wire::decode_protocol_error(&value).ok());
    match parsed {
        Some(error) => TypedError {
            code: error.code.as_str().to_owned(),
            message: error.message,
            retryable: error.retryable,
        },
        None => TypedError::new(
            wire::ProtocolErrorCode::Internal,
            format!("host returned an unexpected error (HTTP {status})"),
        ),
    }
}

/// Tauri command: one unary protocol call over the local Host IPC endpoint.
/// Never rejects; every outcome arrives as [`IpcResponse`]. The overall
/// budget matches the stamped deadline plus the receive grace, so a silent
/// Host surfaces as a typed `DEADLINE_EXCEEDED` instead of a hang.
#[tauri::command]
pub async fn host_ipc_request(args: UnaryArgs) -> IpcResponse {
    let (endpoint, token) = match discover_host() {
        Ok(pair) => pair,
        Err(error) => return IpcResponse::failure(to_typed_discovery_error(&error)),
    };
    let cancel_rx = register_cancel(args.request_id);
    let budget = Duration::from_millis(DEFAULT_RPC_BUDGET_MS + CLIENT_TIMEOUT_GRACE_MS);
    let outcome = tokio::select! {
        biased;
        _ = cancel_rx => None,
        result = tokio::time::timeout(budget, execute_unary(&endpoint, &args, &token)) => {
            deregister_cancel(args.request_id);
            match result {
                Ok(response) => Some(response),
                Err(_elapsed) => Some(IpcResponse::failure(TypedError::new(
                    wire::ProtocolErrorCode::DeadlineExceeded,
                    "the Host did not answer within the request budget",
                ))),
            }
        }
    };
    deregister_cancel(args.request_id);
    outcome.unwrap_or_else(|| {
        IpcResponse::failure(TypedError::new(
            wire::ProtocolErrorCode::Cancelled,
            "the request was cancelled by the caller",
        ))
    })
}

/// Tauri command: aborts one in-flight [`host_ipc_request`].
#[tauri::command]
pub async fn host_ipc_cancel(request_id: u64) -> bool {
    cancel_ipc_request(request_id)
}

fn to_typed_discovery_error(error: &DiscoveryError) -> TypedError {
    let message = error.to_string();
    match error {
        DiscoveryError::MissingToken
        | DiscoveryError::EmptyTokenEnv
        | DiscoveryError::EmptyTokenFile => {
            TypedError::new(wire::ProtocolErrorCode::Unauthenticated, message)
        }
        DiscoveryError::NoDataRoot => TypedError::new(wire::ProtocolErrorCode::Internal, message),
        DiscoveryError::EndpointMissing { .. } | DiscoveryError::EndpointCorrupt(_) => {
            TypedError::new(wire::ProtocolErrorCode::Unavailable, message)
        }
    }
}

// ---------------------------------------------------------------------------
// Event subscription pump.
// ---------------------------------------------------------------------------

static EVENTS_PUMP_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Why the event stream ended; delivered to the webview as
/// [`EVENTS_CLOSED_EVENT`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsClosed {
    pub reason: String,
    pub detail: Option<String>,
}

/// Incremental SSE frame extractor: feed raw bytes, receive complete
/// `data:` payloads. Tolerates both LF and CRLF line endings and chunk
/// boundaries anywhere.
#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(frame_end) = find_blank_line(&self.buffer) {
            let consumed = blank_line_length(&self.buffer[frame_end..]);
            let block: Vec<u8> = self.buffer.drain(..frame_end + consumed).collect();
            for line in split_lines(&block[..frame_end]) {
                if let Some(payload) = line.strip_prefix("data:") {
                    let payload = payload.strip_prefix(' ').unwrap_or(payload);
                    if !payload.is_empty() {
                        frames.push(payload.to_owned());
                    }
                }
            }
        }
        frames
    }
}

/// Finds the offset of the blank line terminating the current SSE block.
fn find_blank_line(buffer: &[u8]) -> Option<usize> {
    for index in 0..buffer.len() {
        if buffer[index] != b'\n' {
            continue;
        }
        if index > 0 && buffer[index - 1] == b'\r' && index >= 2 && buffer[index - 2] == b'\n' {
            return Some(index - 2);
        }
        if index > 0 && buffer[index - 1] == b'\n' {
            return Some(index - 1);
        }
    }
    None
}

fn blank_line_length(buffer: &[u8]) -> usize {
    if buffer.starts_with(b"\r\n\r\n") {
        4
    } else {
        2
    }
}

fn split_lines(block: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(block);
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .map(str::to_owned)
        .collect()
}

/// Opens the `system.subscribeEvents` stream and forwards every decoded
/// frame to the webview until the Host closes it. Reconnection policy stays
/// in the TypeScript connection manager: it decides when to call this again.
///
/// Fails immediately when a pump is already active so two streams never run
/// concurrently.
#[tauri::command]
pub async fn host_ipc_open_events(
    app: tauri::AppHandle,
    args: EventsOpenArgs,
) -> Result<(), String> {
    if EVENTS_PUMP_ACTIVE.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err("an event stream is already active".to_owned());
    }
    let pump = async move {
        let outcome = run_events_pump(&app, &args.last_outage_id).await;
        let closed = match outcome {
            Ok(reason) => EventsClosed {
                reason,
                detail: None,
            },
            Err((reason, detail)) => EventsClosed {
                reason,
                detail: Some(detail),
            },
        };
        let _ = app.emit(EVENTS_CLOSED_EVENT, closed);
    };
    tauri::async_runtime::spawn(async move {
        pump.await;
        EVENTS_PUMP_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    });
    Ok(())
}

type PumpFailure = (String, String);

async fn run_events_pump(
    app: &tauri::AppHandle,
    last_outage_id: &Option<String>,
) -> Result<String, PumpFailure> {
    let (endpoint, token) =
        discover_host().map_err(|error| ("unavailable".to_owned(), error.to_string()))?;

    let stream = dial_async(&endpoint)
        .await
        .map_err(|error| ("unavailable".to_owned(), format!("{error}")))?;

    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|error| ("unavailable".to_owned(), format!("{error}")))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut builder = Request::builder().method("GET").uri("/system/events");
    builder = builder.header(auth::AUTH_METADATA_KEY, bearer_header(&token));
    builder = builder.header(MANIFEST_METADATA_KEY, host_manifest_encoded());
    // Deliberately no deadline header: the subscription lives until the
    // Host drains or the connection breaks.
    if let Some(outage) = last_outage_id.as_deref().filter(|id| !id.is_empty()) {
        builder = builder.header(LAST_OUTAGE_HEADER, outage);
    }
    let request = builder
        .body(request_body(None))
        .map_err(|error| ("internal".to_owned(), error.to_string()))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| ("unavailable".to_owned(), format!("{error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = BodyExt::collect(response.into_body())
            .await
            .map(|collected| String::from_utf8_lossy(&collected.to_bytes()).into_owned())
            .unwrap_or_default();
        return Err((
            "rejected".to_owned(),
            format!(
                "HTTP {status}: {}",
                decode_rejection(&status, &body).message
            ),
        ));
    }

    let mut body = response.into_body();
    let mut parser = SseParser::default();
    loop {
        let frame = body
            .frame()
            .await
            .ok_or(("completed".to_owned(), String::new()))?
            .map_err(|error| ("error".to_owned(), error.to_string()))?;
        if let Some(bytes) = frame.data_ref() {
            for payload in parser.push(bytes) {
                let value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| {
                    (
                        "protocol".to_owned(),
                        format!("the Host sent a malformed event frame: {error}"),
                    )
                })?;
                let _ = app.emit(EVENT_FRAME_EVENT, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "lazarus-desktop-transport-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("auth")).expect("temp root");
        dir
    }

    #[test]
    fn token_selection_prefers_env_then_file_and_never_echoes_secrets() {
        assert!(matches!(
            choose_token(None, None),
            Err(DiscoveryError::MissingToken)
        ));
        assert!(matches!(
            choose_token(Some("  "), None),
            Err(DiscoveryError::EmptyTokenEnv)
        ));
        assert!(matches!(
            choose_token(None, Some("   ")),
            Err(DiscoveryError::EmptyTokenFile)
        ));

        let secret = "s3cret-token-value";
        assert_eq!(choose_token(Some(secret), None).expect("env"), secret);
        assert_eq!(choose_token(None, Some(secret)).expect("file"), secret);
        assert_eq!(
            choose_token(Some("env"), Some("file")).expect("both"),
            "env"
        );
    }

    #[test]
    fn discovery_errors_never_embed_token_values() {
        let root = temp_root("errors");
        std::fs::write(root.join("auth").join("local-token"), "").expect("blank token");

        let error = resolve_token(&root).expect_err("blank file refused");
        let rendered = error.to_string();
        assert!(
            matches!(error, DiscoveryError::EmptyTokenFile),
            "{rendered}"
        );

        let missing = read_endpoint_record(&root).expect("absent record is None");
        assert!(missing.is_none());
        std::fs::create_dir_all(root.join("host")).expect("host dir");
        std::fs::write(root.join("host").join(ENDPOINT_RECORD_FILE), "{not json")
            .expect("corrupt record");
        let corrupt = read_endpoint_record(&root).expect_err("corrupt record refused");
        assert!(matches!(corrupt, DiscoveryError::EndpointCorrupt(_)));
    }

    #[test]
    fn discovery_reads_a_valid_endpoint_record() {
        let root = temp_root("record");
        std::fs::write(root.join("auth").join("local-token"), "tok").expect("token");
        std::fs::create_dir_all(root.join("host")).expect("host dir");
        std::fs::write(
            root.join("host").join(ENDPOINT_RECORD_FILE),
            r#"{"kind":"namedPipe","path":"\\\\.\\pipe\\lazarus-hostd-x"}"#,
        )
        .expect("record");

        let token = resolve_token(&root).expect("token resolves");
        assert_eq!(token, "tok");
        let endpoint = read_endpoint_record(&root)
            .expect("readable")
            .expect("present");
        assert_eq!(endpoint.kind, "namedPipe");
        assert_eq!(endpoint.path, r"\\.\pipe\lazarus-hostd-x");
    }

    #[test]
    fn contract_headers_carry_auth_manifest_and_deadline_without_leaking() {
        let headers = contract_headers("unit-token").expect("valid headers");
        assert_eq!(headers.len(), 3);

        let auth = headers
            .iter()
            .find(|(name, _)| name == auth::AUTH_METADATA_KEY)
            .expect("auth header");
        assert_eq!(auth.1, bearer_header("unit-token"));
        // The token appears only inside the Authorization value; every other
        // header is secret-free.
        for (name, value) in &headers {
            if name != auth::AUTH_METADATA_KEY {
                assert!(!value.contains("unit-token"), "{name} leaked the token");
            }
        }

        let manifest = headers
            .iter()
            .find(|(name, _)| name == MANIFEST_METADATA_KEY)
            .expect("manifest header");
        assert_eq!(manifest.1, host_manifest_encoded());

        let stamp = headers
            .iter()
            .find(|(name, _)| name == deadline::DEADLINE_HEADER)
            .expect("deadline header");
        let now = deadline::unix_now_ms();
        let sent: u64 = stamp.1.parse().expect("epoch ms");
        assert!(sent > now, "the deadline lies in the future");
        assert!(
            sent - now <= DEFAULT_RPC_BUDGET_MS,
            "stamped with the shared budget"
        );
    }

    #[test]
    fn rejections_decode_through_the_generated_envelope() {
        let conforming = decode_rejection(
            &401,
            r#"{"code":"UNAUTHENTICATED","message":"missing or invalid local token","retryable":false}"#,
        );
        assert_eq!(conforming.code, "UNAUTHENTICATED");
        assert!(!conforming.retryable);

        let retryable = decode_rejection(
            &503,
            r#"{"code":"UNAVAILABLE","message":"starting","retryable":true}"#,
        );
        assert_eq!(retryable.code, "UNAVAILABLE");
        assert!(retryable.retryable);

        // Off-contract bodies fall back to a generic INTERNAL error instead
        // of being trusted.
        let fallback = decode_rejection(&500, "<html>boom</html>");
        assert_eq!(fallback.code, "INTERNAL");
        assert!(fallback.message.contains("500"));
    }

    #[test]
    fn sse_parser_survives_arbitrary_chunk_boundaries() {
        let mut parser = SseParser::default();
        // Feed byte-by-byte across LF and CRLF endings; exactly one space
        // after the colon is stripped per the SSE framing rules.
        let full = b"data: {\"a\":1}\r\n\r\ndata:{\"b\":2}\n\ndata: {\"c\":3}\r\n\r\n";
        let mut frames = Vec::new();
        for byte in full.iter() {
            frames.extend(parser.push(std::slice::from_ref(byte)));
        }
        assert_eq!(
            frames,
            vec![
                "{\"a\":1}".to_owned(),
                "{\"b\":2}".to_owned(),
                "{\"c\":3}".to_owned()
            ]
        );

        // An incomplete trailing block waits for more bytes.
        assert!(parser.push(b"data: {\"par").is_empty());
        assert_eq!(
            parser.push(b"tial\"}\r\n\r\n"),
            vec!["{\"partial\"}".to_owned()]
        );
    }

    #[tokio::test]
    async fn cancellation_registry_round_trips() {
        let receiver = register_cancel(9001);
        assert!(cancel_ipc_request(9001));
        assert_eq!(receiver.await, Ok(()));
        assert!(!cancel_ipc_request(9001), "second cancel finds nothing");
        // Cancelling an unknown id is a clean no-op.
        assert!(!cancel_ipc_request(999_999));
    }
}
