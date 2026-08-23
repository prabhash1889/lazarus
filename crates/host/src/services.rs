//! Minimal Axum JSON/HTTP serving for the Phase 1.5 Host unary surface plus
//! the authenticated SSE event subscription.

use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Json;
use axum::Router;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{self, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_core::Stream;
use protocol_rs::auth;
use protocol_rs::bridges::{apply_bridge_steps, downgrade_response_steps};
use protocol_rs::manifest::{
    self as manifest_contract, MethodManifest, NegotiatedManifest, Resolution,
    host_manifest_encoded, negotiate_with_host,
};
use serde::Serialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::HostState;
use crate::events::{EventFrame, needs_tombstone};

/// Header a reconnecting client may send naming the last outage it observed;
/// an exact match suppresses the tombstone resend.
pub const LAST_OUTAGE_HEADER: &str = "x-lazarus-last-outage-id";

/// Serves the unary Host surface over loopback-only JSON/HTTP. Every request
/// passes through [`transport_gate`] before any handler logic runs.
#[derive(Clone)]
pub struct HostServices {
    state: Arc<HostState>,
    token: Arc<str>,
}

impl HostServices {
    pub fn new(state: Arc<HostState>, token: Arc<str>) -> Self {
        Self { state, token }
    }
}

/// The stable error code carried in every typed JSON error body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateCode {
    Unauthenticated,
    InvalidArgument,
    IncompatibleMethodManifest,
}

impl GateCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::IncompatibleMethodManifest => "INCOMPATIBLE_METHOD_MANIFEST",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::InvalidArgument => StatusCode::BAD_REQUEST,
            Self::IncompatibleMethodManifest => StatusCode::PRECONDITION_FAILED,
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
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for GateError {
    fn into_response(self) -> Response {
        (
            self.code.status(),
            Json(ErrorBody {
                code: self.code.as_str(),
                message: self.message,
            }),
        )
            .into_response()
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
        "/workspaces" => Some("workspace.list"),
        "/tasks" => Some("task.list"),
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
/// with this Host's complete encoded manifest attached to every successful
/// response.
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

    let mut response = next.run(request).await;
    if response.status().is_success()
        && let Ok(value) = HeaderValue::from_str(host_manifest_encoded())
    {
        response
            .headers_mut()
            .insert(manifest_contract::MANIFEST_METADATA_KEY, value);
    }
    response
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfoBody {
    host_version: &'static str,
    capabilities: HashMap<String, bool>,
}

async fn system_info(State(services): State<HostServices>) -> Json<SystemInfoBody> {
    Json(SystemInfoBody {
        host_version: env!("CARGO_PKG_VERSION"),
        capabilities: services.state.host_capabilities().clone(),
    })
}

#[derive(Serialize)]
struct HealthBody {
    status: &'static str,
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "SERVING" })
}

#[derive(Serialize)]
struct ListWorkspacesBody {
    workspaces: Vec<serde_json::Value>,
}

async fn list_workspaces() -> Json<ListWorkspacesBody> {
    Json(ListWorkspacesBody {
        workspaces: Vec::new(),
    })
}

/// Unix epoch milliseconds, the v1.1+ `task.list` page timestamp.
fn unix_now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_millis()
}

/// `GET /tasks`: serves `task.list` at this Host's minor, always including
/// the v1.1+ additive `servedAtUnixMs` page timestamp. When negotiation
/// resolved an older bridged peer minor, the declared bridge steps adapt the
/// response down before it is returned; current or newer peers receive the
/// full shape.
async fn list_tasks(negotiated: Option<Extension<NegotiatedMinor>>) -> Json<serde_json::Value> {
    let mut body = serde_json::json!({
        "tasks": [],
        "servedAtUnixMs": unix_now_ms(),
    });
    if let Some(Extension(NegotiatedMinor(minor))) = negotiated {
        apply_bridge_steps(&mut body, downgrade_response_steps("task.list", minor));
    }
    Json(body)
}

/// The SSE subscription stream: the opening tombstone/snapshot prefix, then
/// live frames until the client disconnects or falls behind. The feed is the
/// bus's bounded broadcast channel, so a slow subscriber can never grow
/// memory; lag closes the stream instead of skipping frames, and the client
/// resubscribes for a fresh authoritative snapshot.
struct Subscription {
    prefix: std::vec::IntoIter<EventFrame>,
    /// Direct bounded-broadcast feed with persistent waker state, so a slow
    /// subscriber can never grow memory.
    stream: BroadcastStream<EventFrame>,
}

impl Subscription {
    /// Polls the next frame to deliver, prefix included. Every queued live
    /// frame flows exactly once: the Phase 1 snapshot is empty and frames
    /// carry no state payload, so none can duplicate it.
    fn poll_next_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<EventFrame>> {
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
/// authoritative snapshot, then live events.
async fn system_events(
    State(services): State<HostServices>,
    headers: HeaderMap,
) -> Sse<Subscription> {
    let last_outage_id = headers
        .get(LAST_OUTAGE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
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
    Sse::new(Subscription {
        prefix: prefix.into_iter(),
        stream: BroadcastStream::new(rx),
    })
}

/// Builds the complete loopback Host router with the transport gate layered
/// over the unary endpoints.
pub fn build_router(services: HostServices) -> Router {
    Router::new()
        .route("/system/info", get(system_info))
        .route("/system/health", get(health))
        .route("/system/events", get(system_events))
        .route("/workspaces", get(list_workspaces))
        .route("/tasks", get(list_tasks))
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
        let current = list_tasks(Some(Extension(NegotiatedMinor(2)))).await.0;
        assert!(current.get("servedAtUnixMs").is_some());

        let bridged = list_tasks(Some(Extension(NegotiatedMinor(0)))).await.0;
        assert!(
            bridged.get("servedAtUnixMs").is_none(),
            "the declared 1.0 bridge must strip the additive field"
        );
        assert_eq!(bridged["tasks"], serde_json::json!([]));

        let undeclared = list_tasks(Some(Extension(NegotiatedMinor(1)))).await.0;
        assert!(undeclared.get("servedAtUnixMs").is_some());

        // No negotiation result at all keeps the full shape.
        let plain = list_tasks(None).await.0;
        assert!(plain.get("servedAtUnixMs").is_some());
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
        assert_eq!(rpc_method("/unknown"), None);
    }

    fn bare_subscription(rx: tokio::sync::broadcast::Receiver<EventFrame>) -> Subscription {
        Subscription {
            prefix: Vec::new().into_iter(),
            stream: BroadcastStream::new(rx),
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
}
