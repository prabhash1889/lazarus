//! Minimal Axum JSON/HTTP serving for the Phase 1.5 Host unary surface plus
//! the authenticated SSE event subscription.
//!
//! Every response body is built from (and every caller-supplied payload is
//! decoded against) the generated wire bindings in
//! `protocol_rs::generated_registry::wire`, so neither side can drift from
//! the TypeScript/Zod contract. Caller deadlines are enforced operationally:
//! work stops at the budget and answers the canonical `DEADLINE_EXCEEDED`.

use std::convert::Infallible;
use std::ffi::OsString;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{self, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_core::Stream;
use process_supervisor::{
    CommandSpec, FramedEvent, OutputStream, ProcessEvent, ProcessHandle, ResourceCounters,
    Supervisor, TerminalSize,
};
use protocol_rs::auth;
use protocol_rs::bridges::{apply_bridge_steps, downgrade_response_steps};
use protocol_rs::deadline::{self, Deadline, DeadlineError};
use protocol_rs::generated_registry::wire;
use protocol_rs::manifest::{
    self as manifest_contract, MethodManifest, NegotiatedManifest, Resolution,
    host_manifest_encoded, negotiate_with_host,
};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::HostState;
use crate::events::{EventFrame, needs_tombstone};
use crate::persistence::{PersistenceError, Store, StoredProcess, StoredResourceCounters};
use crate::runtime::DATA_DIR_ENV;

/// Header a reconnecting client may send naming the last outage it observed;
/// an exact match suppresses the tombstone resend.
pub const LAST_OUTAGE_HEADER: &str = "x-lazarus-last-outage-id";

const MAX_GRACEFUL_TIMEOUT_MS: u64 = 5 * 60 * 1_000;

/// Serves the unary Host surface over loopback-only JSON/HTTP. Every request
/// passes through [`transport_gate`] before any handler logic runs.
#[derive(Clone)]
pub struct HostServices {
    state: Arc<HostState>,
    token: Arc<str>,
    processes: Option<Arc<ProcessServices>>,
}

struct ProcessServices {
    store: Arc<Mutex<Store>>,
    supervisor: Supervisor,
}

impl HostServices {
    pub fn new(state: Arc<HostState>, token: Arc<str>) -> Self {
        Self {
            state,
            token,
            processes: None,
        }
    }

    /// Adds the process-supervision runtime used by the `process.*` routes.
    pub fn with_process_supervision(
        state: Arc<HostState>,
        token: Arc<str>,
        store: Arc<Mutex<Store>>,
        supervisor: Supervisor,
    ) -> Self {
        Self {
            state,
            token,
            processes: Some(Arc::new(ProcessServices { store, supervisor })),
        }
    }
}

/// The stable error code carried in every typed JSON error body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateCode {
    AlreadyExists,
    Unauthenticated,
    InvalidArgument,
    IncompatibleMethodManifest,
    DeadlineExceeded,
    NotFound,
    Internal,
}

impl GateCode {
    /// The canonical generated error code this gate rejection carries on the
    /// wire; retryability is derived from it, never set by hand.
    fn error_code(self) -> wire::ProtocolErrorCode {
        match self {
            Self::AlreadyExists => wire::ProtocolErrorCode::AlreadyExists,
            Self::Unauthenticated => wire::ProtocolErrorCode::Unauthenticated,
            Self::InvalidArgument => wire::ProtocolErrorCode::InvalidArgument,
            Self::IncompatibleMethodManifest => wire::ProtocolErrorCode::IncompatibleMethodManifest,
            Self::DeadlineExceeded => wire::ProtocolErrorCode::DeadlineExceeded,
            Self::NotFound => wire::ProtocolErrorCode::NotFound,
            Self::Internal => wire::ProtocolErrorCode::Internal,
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Self::AlreadyExists => StatusCode::CONFLICT,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::InvalidArgument => StatusCode::BAD_REQUEST,
            Self::IncompatibleMethodManifest => StatusCode::PRECONDITION_FAILED,
            Self::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// A typed transport-gate rejection: a stable code plus a human message.
#[derive(Debug, Clone)]
pub struct GateError {
    pub code: GateCode,
    pub message: String,
}

impl GateError {
    fn new(code: GateCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Builds the canonical error envelope for this rejection.
    pub fn to_protocol_error(&self) -> wire::ProtocolError {
        wire::ProtocolError::new(self.code.error_code(), self.message.clone())
    }
}

impl IntoResponse for GateError {
    fn into_response(self) -> Response {
        (self.code.status(), Json(self.to_protocol_error())).into_response()
    }
}

impl From<manifest_contract::ManifestDecodeError> for GateError {
    fn from(err: manifest_contract::ManifestDecodeError) -> Self {
        Self::new(GateCode::InvalidArgument, err.to_string())
    }
}

impl From<manifest_contract::IncompatibleManifest> for GateError {
    fn from(err: manifest_contract::IncompatibleManifest) -> Self {
        Self::new(GateCode::IncompatibleMethodManifest, err.to_string())
    }
}

/// Maps an HTTP route onto its protocol method name for per-method
/// negotiation. Unknown routes still pass authentication first, then fall
/// through to the router's plain 404 without any manifest demand.
fn rpc_method(path: &str) -> Option<&'static str> {
    match path {
        "/system/info" => Some("system.getInfo"),
        "/system/health" => Some("system.health"),
        "/system/events" => Some("system.subscribeEvents"),
        "/system/shutdown" => Some("system.shutdown"),
        "/workspaces" => Some("workspace.list"),
        "/tasks" => Some("task.list"),
        "/process/start" => Some("process.start"),
        "/process/stop" => Some("process.stop"),
        "/process/list" => Some("process.list"),
        "/process/output" => Some("process.output"),
        _ => None,
    }
}

/// The negotiated minor for the method this request is about to hit,
/// inserted by [`transport_gate`] once the peer manifest has passed
/// negotiation. Handlers read it to decide whether a declared bridge must
/// adapt the response down to an older peer minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedMinor(pub u32);

/// The transport gate shared by every unary endpoint: authenticate against
/// the per-install local token first, then decode and negotiate the peer's
/// per-method manifest. Missing or malformed manifests are typed
/// INVALID_ARGUMENT; an incompatible required manifest is a typed
/// INCOMPATIBLE_METHOD_MANIFEST naming only the offending methods.
///
/// The error message deliberately omits the presented token so secrets can
/// never leak through failure paths.
pub fn authorize_and_negotiate(
    token: &str,
    headers: &HeaderMap,
) -> Result<NegotiatedManifest, GateError> {
    authenticate(token, headers)?;
    negotiate_manifest(headers)
}

/// Verifies the Bearer local token. This runs before any other gate logic,
/// including route resolution: an unauthenticated caller must never learn
/// whether a path this Host serves even exists.
fn authenticate(token: &str, headers: &HeaderMap) -> Result<(), GateError> {
    let provided = headers
        .get(auth::AUTH_METADATA_KEY)
        .and_then(|value| value.to_str().ok());
    if !auth::verify_bearer_header(token, provided) {
        return Err(GateError::new(
            GateCode::Unauthenticated,
            "missing or invalid local token",
        ));
    }
    Ok(())
}

/// Decodes and negotiates the peer's method manifest once the caller is
/// authenticated. Only ever reached for routes that map onto a protocol
/// method; unknown paths never demand a manifest.
fn negotiate_manifest(headers: &HeaderMap) -> Result<NegotiatedManifest, GateError> {
    let Some(raw) = headers
        .get(manifest_contract::MANIFEST_METADATA_KEY)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(GateError::new(
            GateCode::InvalidArgument,
            format!(
                "{} header is missing; send your complete method manifest on every request",
                manifest_contract::MANIFEST_METADATA_KEY
            ),
        ));
    };
    let peer: MethodManifest = raw.parse()?;
    let negotiated = negotiate_with_host(&peer)?;
    Ok(negotiated)
}

/// Axum middleware enforcing the transport gate on every request: Bearer
/// authentication first (unknown routes included, so route probing is never
/// unauthenticated), then per-method manifest negotiation for known paths,
/// then the caller's cancellation/deadline budget, with this Host's complete
/// encoded manifest attached to every successful response.
pub async fn transport_gate(
    State(services): State<HostServices>,
    mut request: Request,
    next: Next,
) -> Response {
    // Authentication precedes everything, including route resolution: an
    // unknown route must never answer an unauthenticated caller, or the 404
    // itself becomes a discovery oracle.
    if let Err(gate_error) = authenticate(&services.token, request.headers()) {
        return gate_error.into_response();
    }
    // Unknown routes stay a plain 404 for authenticated callers: no method
    // manifest is demanded of a path this Host does not serve.
    let Some(method) = rpc_method(request.uri().path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let negotiated = match negotiate_manifest(request.headers()) {
        Ok(negotiated) => negotiated,
        Err(gate_error) => return gate_error.into_response(),
    };
    if let Some((_, Resolution::Supported { minor })) =
        negotiated.methods.iter().find(|(m, _)| *m == method)
    {
        request.extensions_mut().insert(NegotiatedMinor(*minor));
    }
    let remaining_ms = match caller_deadline_remaining(request.headers()) {
        Ok(remaining) => remaining,
        Err(gate_error) => return gate_error.into_response(),
    };

    // The handler future runs under the caller's remaining budget. Elapsing
    // drops the future mid-flight - stopping whatever the endpoint was doing
    // - and answers with the canonical DEADLINE_EXCEEDED rejection.
    let response = run_within_deadline(remaining_ms, async { next.run(request).await }, || {
        GateError::new(
            GateCode::DeadlineExceeded,
            "the request's deadline elapsed before it completed",
        )
    })
    .await;

    let mut response = match response {
        Ok(response) => response,
        Err(gate_error) => return gate_error.into_response(),
    };
    if response.status().is_success()
        && let Ok(value) = HeaderValue::from_str(host_manifest_encoded())
    {
        response
            .headers_mut()
            .insert(manifest_contract::MANIFEST_METADATA_KEY, value);
    }
    response
}

/// Reads and validates the caller's optional [`deadline::DEADLINE_HEADER`],
/// returning the remaining budget in milliseconds. A malformed header is a
/// typed `INVALID_ARGUMENT`; an already-elapsed one is an immediate typed
/// `DEADLINE_EXCEEDED` so cancellation stays observable even when no work
/// remains.
pub fn caller_deadline_remaining(headers: &HeaderMap) -> Result<Option<u64>, GateError> {
    let Some(raw) = headers
        .get(deadline::DEADLINE_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    let parsed = Deadline::parse(raw, deadline::unix_now_ms()).map_err(|error| match error {
        DeadlineError::Malformed => GateError::new(
            GateCode::InvalidArgument,
            format!(
                "{} must be Unix epoch milliseconds",
                deadline::DEADLINE_HEADER
            ),
        ),
        DeadlineError::Expired { .. } => GateError::new(
            GateCode::DeadlineExceeded,
            "the request arrived after its deadline had already elapsed",
        ),
    })?;
    Ok(Some(parsed.remaining_ms(deadline::unix_now_ms())))
}

/// Runs `work` under an optional millisecond budget. Elapsing drops the
/// work future - cancelling it - and yields the error produced by
/// `on_elapsed`. Without a budget the work runs to completion.
async fn run_within_deadline<T, F>(
    remaining_ms: Option<u64>,
    work: F,
    on_elapsed: impl FnOnce() -> GateError,
) -> Result<T, GateError>
where
    F: Future<Output = T>,
{
    let Some(remaining_ms) = remaining_ms else {
        return Ok(work.await);
    };
    match tokio::time::timeout(Duration::from_millis(remaining_ms.max(1)), work).await {
        Ok(output) => Ok(output),
        Err(_elapsed) => Err(on_elapsed()),
    }
}

async fn system_info(State(services): State<HostServices>) -> Json<wire::SystemGetInfoResponse> {
    Json(wire::SystemGetInfoResponse {
        host_version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities: services.state.host_capabilities().clone(),
    })
}

async fn health(State(services): State<HostServices>) -> Json<wire::SystemHealthResponse> {
    Json(wire::SystemHealthResponse {
        status: if services.state.is_serving() {
            wire::SystemHealthResponseStatus::Serving
        } else {
            wire::SystemHealthResponseStatus::NotServing
        },
    })
}

/// `POST /system/shutdown`: the authenticated local lifecycle control the
/// CLI (and, through it, the Desktop) uses to stop the Host. It triggers the
/// same graceful drain as a terminal signal: serving flips off, subscribers
/// are closed, supervised processes finalize, and the crash marker clears.
/// The response returns before draining so the caller observes an ack
/// instead of a dropped connection.
async fn system_shutdown(State(services): State<HostServices>) -> Json<serde_json::Value> {
    tracing::info!(
        component = "hostd",
        event = "host.shutdown_requested",
        "graceful shutdown requested over the authenticated local surface"
    );
    services.state.begin_shutdown();
    Json(serde_json::json!({ "status": "SHUTDOWN_REQUESTED" }))
}

/// Decodes a paginated request's query string into its generated request
/// type and enforces the contract's constraints. Malformed values or
/// bound violations are typed `INVALID_ARGUMENT`s, so clients can never
/// believe an out-of-contract request was accepted.
async fn list_workspaces(
    query: Result<Query<wire::WorkspaceListRequest>, QueryRejection>,
) -> Result<Json<wire::WorkspaceListResponse>, GateError> {
    let Query(request) = query
        .map_err(|_| GateError::new(GateCode::InvalidArgument, "query parameters are malformed"))?;
    request.validate().map_err(|reason| {
        GateError::new(
            GateCode::InvalidArgument,
            format!("request violates the method contract: {reason}"),
        )
    })?;
    Ok(Json(wire::WorkspaceListResponse {
        workspaces: Vec::new(),
        pagination: None,
    }))
}

/// `GET /tasks`: serves `task.list` at this Host's minor, always including
/// the v1.1+ additive `servedAtUnixMs` page timestamp. When negotiation
/// resolved an older bridged peer minor, the declared bridge steps adapt the
/// response down before it is returned; current or newer peers receive the
/// full shape.
async fn list_tasks(
    negotiated: Option<Extension<NegotiatedMinor>>,
    query: Result<Query<wire::TaskListRequest>, QueryRejection>,
) -> Result<Json<serde_json::Value>, GateError> {
    let Query(page) = query
        .map_err(|_| GateError::new(GateCode::InvalidArgument, "query parameters are malformed"))?;
    page.validate().map_err(|reason| {
        GateError::new(
            GateCode::InvalidArgument,
            format!("request violates the method contract: {reason}"),
        )
    })?;
    // Typed construction keeps the served payload on-contract by
    // construction; only the declared bridge may reshape it afterwards.
    let mut body = serde_json::to_value(&wire::TaskListResponse {
        tasks: Vec::new(),
        pagination: None,
        served_at_unix_ms: Some(deadline::unix_now_ms()),
    })
    .expect("generated response serializes");
    if let Some(Extension(NegotiatedMinor(minor))) = negotiated {
        apply_bridge_steps(&mut body, downgrade_response_steps("task.list", minor));
    }
    Ok(Json(body))
}

fn process_runtime(services: &HostServices) -> Result<Arc<ProcessServices>, GateError> {
    services.processes.clone().ok_or_else(|| {
        GateError::new(
            GateCode::Internal,
            "process supervision is not configured for this Host",
        )
    })
}

fn process_persistence_error(error: PersistenceError) -> GateError {
    let code = match &error {
        PersistenceError::Sqlite {
            source: rusqlite::Error::SqliteFailure(failure, _),
            ..
        } if failure.code == rusqlite::ErrorCode::ConstraintViolation => GateCode::AlreadyExists,
        _ => GateCode::Internal,
    };
    GateError::new(code, format!("process persistence failed: {error}"))
}

fn lock_process_store(
    processes: &ProcessServices,
) -> Result<std::sync::MutexGuard<'_, Store>, GateError> {
    processes
        .store
        .lock()
        .map_err(|_| GateError::new(GateCode::Internal, "process persistence lock is poisoned"))
}

fn invalid_process_request(reason: impl Into<String>) -> GateError {
    GateError::new(
        GateCode::InvalidArgument,
        format!("request violates the method contract: {}", reason.into()),
    )
}

/// Starts one process tree after its durable `STARTING` record commits.
async fn start_process(
    State(services): State<HostServices>,
    payload: Result<Json<wire::ProcessStartRequest>, JsonRejection>,
) -> Result<Json<wire::ProcessStartResponse>, GateError> {
    let Json(request) =
        payload.map_err(|_| GateError::new(GateCode::InvalidArgument, "JSON body is malformed"))?;
    request.validate().map_err(invalid_process_request)?;
    if request.program.is_empty() {
        return Err(invalid_process_request("program must not be empty"));
    }
    if request.data_dir.is_empty() {
        return Err(invalid_process_request("dataDir must not be empty"));
    }
    if request.env_allowlist.as_ref().is_some_and(|keys| {
        keys.iter()
            .any(|key| key.is_empty() || key.contains(['=', '\0']))
    }) {
        return Err(invalid_process_request(
            "envAllowlist contains an invalid environment variable name",
        ));
    }

    let processes = process_runtime(&services)?;
    let args_json = serde_json::to_string(&request.args)
        .map_err(|error| GateError::new(GateCode::Internal, error.to_string()))?;
    {
        let mut store = lock_process_store(&processes)?;
        store
            .insert_supervised_process(
                &request.process_id,
                &request.program,
                &args_json,
                request.cwd.as_deref(),
                request.run_mode.as_str(),
            )
            .map_err(process_persistence_error)?;
    }

    let mut spec = CommandSpec::new(&request.program).args(request.args.iter().map(OsString::from));
    if let Some(cwd) = &request.cwd {
        spec = spec.cwd(cwd);
    }
    if let Some(keys) = &request.env_allowlist {
        for key in keys {
            if let Some(value) = std::env::var_os(key) {
                spec = spec.env(key, value);
            }
        }
    }
    spec = spec.env(DATA_DIR_ENV, &request.data_dir);
    if request.run_mode == wire::ProcessStartRequestRunMode::Pty {
        spec = spec.pty(TerminalSize::default());
    }

    let handle = match processes
        .supervisor
        .start(request.process_id.clone(), spec)
        .await
    {
        Ok(handle) => handle,
        Err(error) => {
            let mut store = lock_process_store(&processes)?;
            store
                .mark_process_finished(
                    &request.process_id,
                    "STOPPED",
                    None,
                    &StoredResourceCounters::default(),
                )
                .map_err(process_persistence_error)?;
            return Err(invalid_process_request(format!(
                "process could not be started: {error}"
            )));
        }
    };
    let running_transition = {
        let mut store = lock_process_store(&processes)?;
        store
            .mark_process_running(&request.process_id, handle.pid())
            .map_err(process_persistence_error)
    };
    if let Err(error) = running_transition {
        let _ = processes.supervisor.stop(&request.process_id).await;
        return Err(error);
    }
    tokio::spawn(persist_process_events(processes, handle));

    Ok(Json(wire::ProcessStartResponse {
        process_id: request.process_id,
        status: wire::ProcessStartResponseStatus::Running,
    }))
}

/// Stops the complete process tree within the caller's optional grace period.
async fn stop_process(
    State(services): State<HostServices>,
    payload: Result<Json<wire::ProcessStopRequest>, JsonRejection>,
) -> Result<Json<wire::ProcessStopResponse>, GateError> {
    let Json(request) =
        payload.map_err(|_| GateError::new(GateCode::InvalidArgument, "JSON body is malformed"))?;
    request.validate().map_err(invalid_process_request)?;
    let grace = match request.graceful_timeout_ms {
        Some(0) => {
            return Err(invalid_process_request(
                "gracefulTimeoutMs must be greater than zero",
            ));
        }
        Some(milliseconds) if milliseconds > MAX_GRACEFUL_TIMEOUT_MS => {
            return Err(invalid_process_request(format!(
                "gracefulTimeoutMs must be at most {MAX_GRACEFUL_TIMEOUT_MS}"
            )));
        }
        Some(milliseconds) => Some(Duration::from_millis(milliseconds)),
        None => None,
    };
    let processes = process_runtime(&services)?;
    let exit = processes
        .supervisor
        .stop_with_timeout(&request.process_id, grace)
        .await
        .map_err(|error| GateError::new(GateCode::NotFound, error.to_string()))?;
    let handle = processes
        .supervisor
        .get(&request.process_id)
        .ok_or_else(|| GateError::new(GateCode::NotFound, "supervised process was not found"))?;
    let counters = stored_counters(&handle.counters());
    let dropped = handle.replay(0).dropped_bytes;
    {
        let mut store = lock_process_store(&processes)?;
        store
            .record_dropped_output_bytes(&request.process_id, dropped)
            .and_then(|()| {
                store.mark_process_finished(&request.process_id, "STOPPED", exit.code, &counters)
            })
            .map_err(process_persistence_error)?;
    }
    Ok(Json(wire::ProcessStopResponse {
        process_id: request.process_id,
        status: wire::ProcessStopResponseStatus::Stopped,
    }))
}

async fn list_processes(
    State(services): State<HostServices>,
) -> Result<Json<Vec<wire::ProcessListResponseItem>>, GateError> {
    let processes = process_runtime(&services)?;
    let stored = lock_process_store(&processes)?
        .list_supervised_processes()
        .map_err(process_persistence_error)?;
    let response = stored
        .iter()
        .map(|process| process_summary(&processes.supervisor, process))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(response))
}

async fn process_output(
    State(services): State<HostServices>,
    query: Result<Query<wire::ProcessOutputRequest>, QueryRejection>,
) -> Result<Json<wire::ProcessOutputResponse>, GateError> {
    let Query(request) = query
        .map_err(|_| GateError::new(GateCode::InvalidArgument, "query parameters are malformed"))?;
    request.validate().map_err(invalid_process_request)?;
    let processes = process_runtime(&services)?;
    let replay = lock_process_store(&processes)?
        .process_output(&request.process_id, request.offset)
        .map_err(process_persistence_error)?
        .ok_or_else(|| GateError::new(GateCode::NotFound, "supervised process was not found"))?;
    let frames = replay
        .frames
        .into_iter()
        .map(|frame| {
            let stream = match frame.stream.as_str() {
                "STDOUT" => wire::ProcessOutputResponseFramesItemStream::Stdout,
                "STDERR" => wire::ProcessOutputResponseFramesItemStream::Stderr,
                "PTY" => wire::ProcessOutputResponseFramesItemStream::Pty,
                _ => {
                    return Err(GateError::new(
                        GateCode::Internal,
                        "stored process output has an invalid stream",
                    ));
                }
            };
            Ok(wire::ProcessOutputResponseFramesItem {
                seq: frame.seq,
                stream,
                payload: BASE64.encode(frame.payload),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(wire::ProcessOutputResponse {
        frames,
        next_offset: replay.next_offset,
        truncated: replay.truncated,
    }))
}

fn process_summary(
    supervisor: &Supervisor,
    process: &StoredProcess,
) -> Result<wire::ProcessListResponseItem, GateError> {
    let handle = supervisor.get(&process.id);
    let (status, counters, exit_code, dropped_output_bytes) = if let Some(handle) = handle {
        let counters = handle.counters();
        let status = if handle.is_running() {
            wire::ProcessListResponseItemStatus::Running
        } else if process.status == "STOPPED" {
            wire::ProcessListResponseItemStatus::Stopped
        } else {
            wire::ProcessListResponseItemStatus::Exited
        };
        let exit_code = counters
            .exit
            .as_ref()
            .and_then(|exit| exit.code)
            .and_then(|code| u64::try_from(code).ok());
        let dropped = process
            .dropped_output_bytes
            .max(handle.replay(0).dropped_bytes);
        (status, wire_counters(&counters), exit_code, dropped)
    } else {
        (
            stored_status(&process.status)?,
            wire::ProcessListResponseItemResourceCounters {
                duration_ms: process.counters.duration_ms,
                stdout_bytes: process.counters.stdout_bytes,
                stderr_bytes: process.counters.stderr_bytes,
                cpu_ms: process.counters.cpu_ms,
                peak_memory_bytes: process.counters.peak_memory_bytes,
            },
            process.exit_code.and_then(|code| u64::try_from(code).ok()),
            process.dropped_output_bytes,
        )
    };
    Ok(wire::ProcessListResponseItem {
        process_id: process.id.clone(),
        status,
        started_at: Some(process.started_at.clone()),
        exited_at: process.exited_at.clone(),
        exit_code,
        resource_counters: counters,
        dropped_output_bytes,
    })
}

fn stored_status(status: &str) -> Result<wire::ProcessListResponseItemStatus, GateError> {
    match status {
        "STARTING" => Ok(wire::ProcessListResponseItemStatus::Starting),
        "RUNNING" => Ok(wire::ProcessListResponseItemStatus::Running),
        "EXITED" => Ok(wire::ProcessListResponseItemStatus::Exited),
        "STOPPED" => Ok(wire::ProcessListResponseItemStatus::Stopped),
        "INTERRUPTED" => Ok(wire::ProcessListResponseItemStatus::Interrupted),
        _ => Err(GateError::new(
            GateCode::Internal,
            "stored process has an invalid status",
        )),
    }
}

fn stored_counters(counters: &ResourceCounters) -> StoredResourceCounters {
    StoredResourceCounters {
        duration_ms: Some(duration_ms(counters.wall_time)),
        stdout_bytes: counters.stdout_bytes.saturating_add(counters.pty_bytes),
        stderr_bytes: counters.stderr_bytes,
        cpu_ms: counters.cpu_time.map(duration_ms),
        peak_memory_bytes: counters.peak_memory_bytes,
    }
}

fn wire_counters(counters: &ResourceCounters) -> wire::ProcessListResponseItemResourceCounters {
    let counters = stored_counters(counters);
    wire::ProcessListResponseItemResourceCounters {
        duration_ms: counters.duration_ms,
        stdout_bytes: counters.stdout_bytes,
        stderr_bytes: counters.stderr_bytes,
        cpu_ms: counters.cpu_ms,
        peak_memory_bytes: counters.peak_memory_bytes,
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

async fn persist_process_events(processes: Arc<ProcessServices>, handle: ProcessHandle) {
    let mut events = handle.subscribe();
    let mut next_offset = 0;
    loop {
        let replay = handle.replay(next_offset);
        if let Err(error) = persist_dropped_output(&processes, handle.id(), replay.dropped_bytes) {
            tracing::warn!(component = "hostd", event = "process.output_persist_failed", message = %error.message);
            return;
        }
        for frame in replay.frames {
            if frame.offset < next_offset {
                continue;
            }
            next_offset = frame.offset.saturating_add(1);
            if let Err(error) = persist_process_frame(&processes, handle.id(), &frame) {
                tracing::warn!(component = "hostd", event = "process.output_persist_failed", message = %error.message);
                return;
            }
        }
        next_offset = next_offset.max(replay.next_offset);
        if !handle.is_running() {
            break;
        }
        match events.recv().await {
            Ok(frame) if frame.offset >= next_offset => {
                next_offset = frame.offset.saturating_add(1);
                if let Err(error) = persist_process_frame(&processes, handle.id(), &frame) {
                    tracing::warn!(component = "hostd", event = "process.output_persist_failed", message = %error.message);
                    return;
                }
            }
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }

    let counters = handle.counters();
    if let Err(error) = lock_process_store(&processes).and_then(|mut store| {
        store
            .record_dropped_output_bytes(handle.id(), handle.replay(0).dropped_bytes)
            .and_then(|()| {
                store.mark_process_finished(
                    handle.id(),
                    "EXITED",
                    counters.exit.as_ref().and_then(|exit| exit.code),
                    &stored_counters(&counters),
                )
            })
            .map_err(process_persistence_error)
    }) {
        tracing::warn!(component = "hostd", event = "process.exit_persist_failed", message = %error.message);
    }
}

fn persist_dropped_output(
    processes: &ProcessServices,
    process_id: &str,
    dropped_bytes: u64,
) -> Result<(), GateError> {
    lock_process_store(processes)?
        .record_dropped_output_bytes(process_id, dropped_bytes)
        .map_err(process_persistence_error)
}

fn persist_process_frame(
    processes: &ProcessServices,
    process_id: &str,
    frame: &FramedEvent,
) -> Result<(), GateError> {
    let ProcessEvent::Output { stream, bytes } = &frame.event else {
        return Ok(());
    };
    let stream = match stream {
        OutputStream::Stdout => "STDOUT",
        OutputStream::Stderr => "STDERR",
        OutputStream::Pty => "PTY",
    };
    lock_process_store(processes)?
        .append_output_frame(process_id, frame.offset, stream, bytes)
        .map_err(process_persistence_error)
}

/// The SSE subscription stream: the opening tombstone/snapshot prefix, then
/// live frames until the client disconnects, falls behind, or reaches its
/// deadline. The feed is the bus's bounded broadcast channel, so a slow
/// subscriber can never grow memory; lag closes the stream instead of
/// skipping frames, and the client resubscribes for a fresh authoritative
/// snapshot.
struct Subscription {
    prefix: std::vec::IntoIter<EventFrame>,
    /// Direct bounded-broadcast feed with persistent waker state, so a slow
    /// subscriber can never grow memory.
    stream: BroadcastStream<EventFrame>,
    /// Closes deadline-free streams when the Host starts graceful shutdown.
    shutdown: Option<BroadcastStream<()>>,
    /// Timer that wakes this stream at the caller deadline even when the
    /// event feed is idle.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Subscription {
    /// Polls the next frame to deliver, prefix included. Every queued live
    /// frame flows exactly once: the Phase 1 snapshot is empty and frames
    /// carry no state payload, so none can duplicate it.
    fn poll_next_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<EventFrame>> {
        if let Some(shutdown) = &mut self.shutdown {
            match Pin::new(shutdown).poll_next(cx) {
                Poll::Ready(Some(Ok(())) | Some(Err(_)) | None) => return Poll::Ready(None),
                Poll::Pending => {}
            }
        }
        if let Some(timer) = self.deadline.as_mut()
            && timer.as_mut().poll(cx).is_ready()
        {
            return Poll::Ready(None);
        }
        if let Some(frame) = self.prefix.next() {
            return Poll::Ready(Some(frame));
        }
        let this = self.get_mut();
        match std::task::ready!(Pin::new(&mut this.stream).poll_next(cx)) {
            Some(Ok(frame)) => Poll::Ready(Some(frame)),
            // Lag means frames were skipped: close the stream rather
            // than fake continuity; None means the bus is gone. Either
            // way the client resubscribes for a fresh snapshot.
            Some(Err(BroadcastStreamRecvError::Lagged(_))) | None => Poll::Ready(None),
        }
    }
}

impl Stream for Subscription {
    type Item = Result<sse::Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next_frame(cx)
            .map(|option| option.map(|frame| Ok(encode_frame(frame))))
    }
}

fn encode_frame(frame: EventFrame) -> sse::Event {
    sse::Event::default().data(frame.encode())
}

/// `GET /system/events`: authenticated SSE subscription. Sends the restart
/// tombstone exactly once per outage (skipped when the client's
/// [`LAST_OUTAGE_HEADER`] already names the current outage), then an
/// authoritative snapshot, then live events. A caller deadline closes the
/// stream when it elapses; the client resubscribes.
async fn system_events(
    State(services): State<HostServices>,
    headers: HeaderMap,
) -> Result<Sse<Subscription>, GateError> {
    let last_outage_id = headers
        .get(LAST_OUTAGE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let deadline = caller_deadline_remaining(&headers)?
        .map(|remaining| Box::pin(tokio::time::sleep(Duration::from_millis(remaining.max(1)))));
    let bus = &services.state.bus;
    // The Phase 1 snapshot is empty and live frames carry no state payload,
    // so every frame queued from this subscription onward must be delivered
    // exactly once; there is nothing to dedupe against the snapshot.
    let rx = bus.subscribe();
    let mut prefix = Vec::new();
    if needs_tombstone(last_outage_id.as_deref(), bus.outage_id()) {
        prefix.push(EventFrame::tombstone(bus.outage_id()));
    }
    prefix.push(EventFrame::authoritative_snapshot());
    Ok(Sse::new(Subscription {
        prefix: prefix.into_iter(),
        stream: BroadcastStream::new(rx),
        shutdown: Some(BroadcastStream::new(services.state.subscribe_shutdown())),
        deadline,
    }))
}

/// Builds the complete loopback Host router with the transport gate layered
/// over the unary endpoints.
pub fn build_router(services: HostServices) -> Router {
    Router::new()
        .route("/system/info", get(system_info))
        .route("/system/health", get(health))
        .route("/system/events", get(system_events))
        .route("/system/shutdown", post(system_shutdown))
        .route("/workspaces", get(list_workspaces))
        .route("/tasks", get(list_tasks))
        .route("/process/start", post(start_process))
        .route("/process/stop", post(stop_process))
        .route("/process/list", get(list_processes))
        .route("/process/output", get(process_output))
        .layer(axum::middleware::from_fn_with_state(
            services.clone(),
            transport_gate,
        ))
        .with_state(services)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol_rs::auth::bearer_header;
    use protocol_rs::manifest::{MethodManifest, Resolution, host_manifest};

    const TOKEN: &str = "unit-test-token";

    fn gated_headers(manifest: Option<&str>, authorization: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(auth) = authorization {
            headers.insert(
                auth::AUTH_METADATA_KEY,
                HeaderValue::from_str(auth).expect("valid header"),
            );
        }
        if let Some(raw) = manifest {
            headers.insert(
                manifest_contract::MANIFEST_METADATA_KEY,
                HeaderValue::from_str(raw).expect("valid header"),
            );
        }
        headers
    }

    fn authed_headers(manifest: Option<&str>) -> HeaderMap {
        gated_headers(manifest, Some(&bearer_header(TOKEN)))
    }

    #[test]
    fn valid_manifest_is_negotiated() {
        let peer = host_manifest();
        let negotiated = authorize_and_negotiate(TOKEN, &authed_headers(Some(&peer.to_string())))
            .expect("gate passes");
        assert_eq!(negotiated.methods.len(), peer.len());
        for (name, resolution) in &negotiated.methods {
            let minor = protocol_rs::generated_registry::binding_by_name(name)
                .expect("generated binding")
                .minor;
            assert_eq!(
                resolution,
                &Resolution::Supported { minor },
                "{name} supported at the host minor"
            );
        }
    }

    /// The v1.1+ `servedAtUnixMs` page timestamp reaches current peers, is
    /// stripped for the bridged 1.0 peer, and survives only because the
    /// undeclared-minor case can never reach a handler (the gate refuses
    /// it with 412 before `list_tasks` runs).
    #[tokio::test]
    async fn task_list_adapts_only_for_the_bridged_older_minor() {
        let empty_query = || {
            Ok(Query(wire::TaskListRequest {
                cursor: None,
                page_size: None,
            }))
        };
        let current = list_tasks(Some(Extension(NegotiatedMinor(2))), empty_query())
            .await
            .expect("in-contract request")
            .0;
        assert!(current.get("servedAtUnixMs").is_some());

        let bridged = list_tasks(Some(Extension(NegotiatedMinor(0))), empty_query())
            .await
            .expect("in-contract request")
            .0;
        assert!(
            bridged.get("servedAtUnixMs").is_none(),
            "the declared 1.0 bridge must strip the additive field"
        );
        assert_eq!(bridged["tasks"], serde_json::json!([]));

        let undeclared = list_tasks(Some(Extension(NegotiatedMinor(1))), empty_query())
            .await
            .expect("in-contract request")
            .0;
        assert!(undeclared.get("servedAtUnixMs").is_some());

        // No negotiation result at all keeps the full shape.
        let plain = list_tasks(None, empty_query())
            .await
            .expect("in-contract request")
            .0;
        assert!(plain.get("servedAtUnixMs").is_some());
    }

    #[tokio::test]
    async fn out_of_contract_page_requests_are_typed_invalid_arguments() {
        // pageSize below the contracted floor decodes but violates
        // validation, so the request is rejected before any work runs.
        let query = Ok(Query(wire::TaskListRequest {
            cursor: None,
            page_size: Some(0),
        }));
        let rejected = list_tasks(None, query).await;
        let error = rejected.unwrap_err();
        assert_eq!(error.code, GateCode::InvalidArgument);
        assert!(error.message.contains("pageSize"), "{error:?}");
    }

    /// Cancellation is operational: a deadline that elapses drops the work
    /// future mid-flight instead of letting it finish in the background.
    #[tokio::test]
    async fn an_elapsed_deadline_stops_work() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let completed = Arc::new(AtomicBool::new(false));
        let flag = completed.clone();

        let work = async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            flag.store(true, Ordering::SeqCst);
            "finished"
        };

        // 1 ms of budget against a 30 s job: elapsing cancels the job.
        let outcome = run_within_deadline(Some(1), work, || {
            GateError::new(GateCode::DeadlineExceeded, "elapsed")
        })
        .await;
        assert_eq!(outcome.unwrap_err().code, GateCode::DeadlineExceeded);
        assert!(
            !completed.load(Ordering::SeqCst),
            "the cancelled future must never have completed its work"
        );

        // With enough budget the same shape completes.
        let outcome: Result<&'static str, GateError> =
            run_within_deadline(Some(5_000), async { "finished" }, || unreachable!()).await;
        assert_eq!(outcome.expect("completes within budget"), "finished");
    }

    #[test]
    fn caller_deadlines_parse_validate_and_expose_remaining_budget() {
        let now = deadline::unix_now_ms();

        // No header means no budget: unlimited.
        assert_eq!(caller_deadline_remaining(&HeaderMap::new()).unwrap(), None);

        let mut headers = HeaderMap::new();
        headers.insert(
            deadline::DEADLINE_HEADER,
            HeaderValue::from_str(&Deadline::header_from_budget(now, 250)).expect("valid"),
        );
        let remaining = caller_deadline_remaining(&headers)
            .expect("future deadline")
            .expect("some budget");
        assert!(
            (200..=250).contains(&remaining),
            "deadline stamps the shared budget: {remaining}"
        );

        // Malformed values are typed INVALID_ARGUMENT.
        let mut headers = HeaderMap::new();
        headers.insert(deadline::DEADLINE_HEADER, HeaderValue::from_static("soon"));
        let error = caller_deadline_remaining(&headers).unwrap_err();
        assert_eq!(error.code, GateCode::InvalidArgument);

        // An elapsed deadline is an immediate typed DEADLINE_EXCEEDED.
        let mut headers = HeaderMap::new();
        headers.insert(
            deadline::DEADLINE_HEADER,
            HeaderValue::from_str(&now.saturating_sub(1_000).to_string()).expect("valid"),
        );
        let error = caller_deadline_remaining(&headers).unwrap_err();
        assert_eq!(error.code, GateCode::DeadlineExceeded);
        assert!(
            error.to_protocol_error().retryable,
            "the canonical classification marks deadline overruns retryable"
        );
    }

    #[test]
    fn gate_rejections_carry_the_canonical_error_envelope() {
        let unauthenticated = GateError::new(GateCode::Unauthenticated, "missing token");
        let envelope = unauthenticated.to_protocol_error();
        assert_eq!(envelope.code.as_str(), "UNAUTHENTICATED");
        assert!(!envelope.retryable);

        let deadline_error =
            GateError::new(GateCode::DeadlineExceeded, "budget elapsed").to_protocol_error();
        assert_eq!(deadline_error.code.as_str(), "DEADLINE_EXCEEDED");
        assert!(
            deadline_error.retryable,
            "the canonical classification marks deadlines retryable"
        );

        // Serialization matches the wire contract byte for byte.
        let rendered =
            serde_json::to_value(unauthenticated.to_protocol_error()).expect("serializes");
        assert_eq!(
            rendered,
            serde_json::json!({
                "code": "UNAUTHENTICATED",
                "message": "missing token",
                "retryable": false,
            })
        );
    }

    #[test]
    fn bad_or_missing_auth_is_unauthenticated_without_leaking_the_token() {
        let rejected =
            authorize_and_negotiate(TOKEN, &gated_headers(None, None)).expect_err("missing header");
        assert_eq!(rejected.code, GateCode::Unauthenticated);
        assert_eq!(rejected.code.status(), StatusCode::UNAUTHORIZED);

        let rejected = authorize_and_negotiate(
            TOKEN,
            &gated_headers(None, Some(&bearer_header("not-the-token"))),
        )
        .expect_err("wrong token");
        assert_eq!(rejected.code, GateCode::Unauthenticated);

        // Well-formed value but missing the Bearer scheme.
        let rejected =
            authorize_and_negotiate(TOKEN, &gated_headers(None, Some(TOKEN))).expect_err("bare");
        assert_eq!(rejected.code, GateCode::Unauthenticated);

        for rejection in [
            authorize_and_negotiate(TOKEN, &gated_headers(None, None)).unwrap_err(),
            authorize_and_negotiate(TOKEN, &gated_headers(None, Some(&bearer_header("nope"))))
                .unwrap_err(),
            authorize_and_negotiate(TOKEN, &gated_headers(None, Some(TOKEN))).unwrap_err(),
        ] {
            assert!(
                !rejection.message.contains(TOKEN),
                "error must not echo the token"
            );
        }
    }

    #[test]
    fn missing_or_malformed_manifest_is_invalid_argument() {
        let missing =
            authorize_and_negotiate(TOKEN, &authed_headers(None)).expect_err("no manifest");
        assert_eq!(missing.code, GateCode::InvalidArgument);

        let malformed = authorize_and_negotiate(TOKEN, &authed_headers(Some("v1:not-an-entry")))
            .expect_err("malformed");
        assert_eq!(malformed.code, GateCode::InvalidArgument);
    }

    #[test]
    fn incompatible_required_manifest_names_only_offenders() {
        // workspace.list carries a wrong major; every other method is fine.
        let mut peer = MethodManifest::default();
        for (name, version) in host_manifest().iter() {
            peer.try_insert(
                name.clone(),
                if name == "workspace.list" {
                    2
                } else {
                    version.major
                },
                version.minor,
            )
            .expect("unique generated method");
        }
        let rejected = authorize_and_negotiate(TOKEN, &authed_headers(Some(&peer.to_string())))
            .expect_err("major mismatch");
        assert_eq!(rejected.code, GateCode::IncompatibleMethodManifest);
        assert_eq!(
            rejected.code.status(),
            StatusCode::PRECONDITION_FAILED,
            "major mismatch maps to 412"
        );
        assert!(rejected.message.contains("workspace.list"));
        assert!(!rejected.message.contains("system.health"));
    }

    #[test]
    fn routes_map_onto_protocol_method_names() {
        assert_eq!(rpc_method("/system/info"), Some("system.getInfo"));
        assert_eq!(rpc_method("/system/health"), Some("system.health"));
        assert_eq!(rpc_method("/workspaces"), Some("workspace.list"));
        assert_eq!(rpc_method("/tasks"), Some("task.list"));
        assert_eq!(rpc_method("/process/start"), Some("process.start"));
        assert_eq!(rpc_method("/process/stop"), Some("process.stop"));
        assert_eq!(rpc_method("/process/list"), Some("process.list"));
        assert_eq!(rpc_method("/process/output"), Some("process.output"));
        assert_eq!(rpc_method("/system/shutdown"), Some("system.shutdown"));
        assert_eq!(rpc_method("/unknown"), None);
    }

    fn bare_subscription(rx: tokio::sync::broadcast::Receiver<EventFrame>) -> Subscription {
        Subscription {
            prefix: Vec::new().into_iter(),
            stream: BroadcastStream::new(rx),
            shutdown: None,
            deadline: None,
        }
    }

    async fn next_frame(sub: &mut Subscription) -> Option<EventFrame> {
        std::future::poll_fn(|cx| Pin::new(&mut *sub).poll_next_frame(cx)).await
    }

    /// Regression for the dropped-frame bug: a frame published after the
    /// subscription is registered is queued by it, and because the Phase 1
    /// snapshot reflects nothing, it must reach the client - the old code
    /// skipped it as "already in the snapshot" purely on sequence order.
    #[tokio::test]
    async fn every_queued_live_frame_is_delivered_exactly_once() {
        let bus = crate::events::EventBus::new();
        let rx = bus.subscribe();
        // Both frames are queued by the subscription; under the old
        // watermark logic both sat below the seam and were dropped.
        let first = bus.publish();
        let second = bus.publish();
        let mut sub = bare_subscription(rx);
        let third = bus.publish();

        assert_eq!(next_frame(&mut sub).await, Some(first));
        assert_eq!(next_frame(&mut sub).await, Some(second));
        assert_eq!(next_frame(&mut sub).await, Some(third));
    }

    /// Overflowing the bounded broadcast buffer terminates the stream
    /// instead of skipping frames or buffering without bound.
    #[tokio::test]
    async fn a_lagged_subscriber_stream_closes_instead_of_skipping_frames() {
        let bus = crate::events::EventBus::with_event_capacity(2);
        let rx = bus.subscribe();
        for _ in 0..4 {
            bus.publish();
        }
        let mut sub = bare_subscription(rx);
        assert_eq!(
            next_frame(&mut sub).await,
            None,
            "lag closes the stream so the client resubscribes"
        );
    }

    /// The opening prefix always precedes any live frame, in order.
    #[tokio::test]
    async fn prefix_frames_are_delivered_before_queued_live_frames() {
        let bus = crate::events::EventBus::new();
        let rx = bus.subscribe();
        let outage_id = bus.outage_id().to_owned();
        bus.publish();
        drop(bus);
        let mut sub = Subscription {
            prefix: vec![
                EventFrame::tombstone(&outage_id),
                EventFrame::authoritative_snapshot(),
            ]
            .into_iter(),
            stream: BroadcastStream::new(rx),
            shutdown: None,
            deadline: None,
        };
        assert_eq!(
            next_frame(&mut sub).await,
            Some(EventFrame::tombstone(&outage_id))
        );
        assert_eq!(
            next_frame(&mut sub).await,
            Some(EventFrame::authoritative_snapshot())
        );
        assert_eq!(
            next_frame(&mut sub).await,
            Some(EventFrame::Live { sequence: 1 })
        );
        assert_eq!(
            next_frame(&mut sub).await,
            None,
            "dropping the bus closes the stream"
        );
    }

    #[tokio::test]
    async fn host_shutdown_closes_a_deadline_free_subscription() {
        let state = crate::HostState::new();
        let bus = crate::events::EventBus::new();
        let mut sub = Subscription {
            prefix: Vec::new().into_iter(),
            stream: BroadcastStream::new(bus.subscribe()),
            shutdown: Some(BroadcastStream::new(state.subscribe_shutdown())),
            deadline: None,
        };
        state.begin_shutdown();
        assert_eq!(next_frame(&mut sub).await, None);
    }
}
