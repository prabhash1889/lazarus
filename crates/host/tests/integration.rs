//! End-to-end Phase 1.5 contract check against an in-process Axum Host
//! server over real loopback HTTP.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use lazarus_hostd::{HostServices, HostState, LAST_OUTAGE_HEADER};
use protocol_rs::auth::{self, bearer_header};
use protocol_rs::deadline::{DEFAULT_RPC_BUDGET_MS, Deadline};
use protocol_rs::generated_registry::wire;
use protocol_rs::manifest::{
    MANIFEST_METADATA_KEY, MethodManifest, host_manifest, host_manifest_encoded,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TEST_TOKEN: &str = "integration-test-token";
const READ_TIMEOUT: Duration = Duration::from_secs(10);

async fn spawn_host() -> (SocketAddr, Arc<HostState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let state = Arc::new(HostState::with_event_capacity(64));
    let services = HostServices::new(state.clone(), Arc::from(TEST_TOKEN));
    let app = lazarus_hostd::build_router(services);
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("host server runs");
    });
    (addr, state)
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("JSON body")
    }
}

/// Issues a raw HTTP/1.1 GET with optional `Authorization` and manifest
/// headers plus any extra contract headers, and reads the full response
/// (the request closes the connection).
async fn get_with_extras(
    addr: SocketAddr,
    path: &str,
    authorization: Option<&str>,
    manifest: Option<&str>,
    extras: &[(&str, String)],
) -> HttpResponse {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to host");
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(auth) = authorization {
        request.push_str(&format!("{}: {auth}\r\n", auth::AUTH_METADATA_KEY));
    }
    if let Some(manifest) = manifest {
        request.push_str(&format!("{MANIFEST_METADATA_KEY}: {manifest}\r\n"));
    }
    for (name, value) in extras {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    parse_response(&raw)
}

/// Issues a raw HTTP/1.1 GET with optional `Authorization` and manifest
/// headers and reads the full response (the request closes the connection).
async fn get(
    addr: SocketAddr,
    path: &str,
    authorization: Option<&str>,
    manifest: Option<&str>,
) -> HttpResponse {
    get_with_extras(addr, path, authorization, manifest, &[]).await
}

fn parse_response(raw: &[u8]) -> HttpResponse {
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response has a header terminator");
    let head = std::str::from_utf8(&raw[..head_end]).expect("response head is ASCII");
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status code");
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    HttpResponse {
        status,
        headers,
        body: raw[head_end + 4..].to_vec(),
    }
}

fn valid_auth_header() -> &'static str {
    static BEARER: OnceLock<String> = OnceLock::new();
    BEARER.get_or_init(|| bearer_header(TEST_TOKEN))
}

/// A fully authenticated request whose manifest matches this Host exactly.
async fn get_authed(addr: SocketAddr, path: &str) -> HttpResponse {
    get(
        addr,
        path,
        Some(valid_auth_header()),
        Some(host_manifest_encoded()),
    )
    .await
}

/// A peer manifest equal to the Host's except for any overridden entries.
fn peer_manifest(entries: &[(String, u32, u32)]) -> String {
    let mut manifest = MethodManifest::default();
    for (name, major, minor) in entries {
        manifest
            .try_insert(name.clone(), *major, *minor)
            .expect("test entry");
    }
    manifest.to_string()
}

fn full_floor_entries() -> Vec<(String, u32, u32)> {
    host_manifest()
        .iter()
        .map(|(name, version)| (name.clone(), version.major, version.minor))
        .collect()
}

fn assert_advertises_host_manifest(response: &HttpResponse) {
    let advertised: MethodManifest = response
        .header(MANIFEST_METADATA_KEY)
        .unwrap_or_else(|| panic!("response advertises the host manifest"))
        .parse()
        .expect("decodable manifest");
    assert_eq!(advertised, host_manifest());
}

#[tokio::test(flavor = "multi_thread")]
async fn phase15_unary_contract_end_to_end() {
    let (addr, _state) = spawn_host().await;

    // Health reports SERVING and advertises the complete Host manifest.
    let health = get_authed(addr, "/system/health").await;
    assert_eq!(health.status, 200);
    assert_advertises_host_manifest(&health);
    assert_eq!(health.body_json()["status"], "SERVING");

    // System info exposes the Host version, capabilities, and (v1.1+) the
    // incarnation start stamp.
    let info = get_authed(addr, "/system/info").await;
    assert_eq!(info.status, 200);
    assert_advertises_host_manifest(&info);
    let info_body = info.body_json();
    assert_eq!(
        info_body["hostVersion"],
        env!("CARGO_PKG_VERSION").to_string()
    );
    assert_eq!(info_body["capabilities"]["events"], true);
    let started_at = info_body["startedAtUnixMs"].as_u64().expect("start stamp");
    assert!(started_at > 0, "the start stamp is a positive epoch ms");

    // A bridged system.getInfo 1.0 peer receives the response without the
    // additive v1.1 field.
    let mut bridged_entries = full_floor_entries();
    for entry in &mut bridged_entries {
        if entry.0 == "system.getInfo" {
            entry.2 = 0;
        }
    }
    let bridged_info = get(
        addr,
        "/system/info",
        Some(valid_auth_header()),
        Some(&peer_manifest(&bridged_entries)),
    )
    .await;
    assert_eq!(bridged_info.status, 200);
    assert_eq!(
        bridged_info.body_json()["startedAtUnixMs"],
        serde_json::Value::Null,
        "the declared 1.0 bridge strips the additive field"
    );

    // Both list endpoints answer with empty stub pages and their manifest.
    let workspaces = get_authed(addr, "/workspaces").await;
    assert_eq!(workspaces.status, 200);
    assert_advertises_host_manifest(&workspaces);
    assert_eq!(workspaces.body_json()["workspaces"], serde_json::json!([]));

    let tasks = get_authed(addr, "/tasks").await;
    assert_eq!(tasks.status, 200);
    assert_advertises_host_manifest(&tasks);
    assert_eq!(tasks.body_json()["tasks"], serde_json::json!([]));

    // Unknown routes are a plain 404 without leaking internals.
    let unknown = get(
        addr,
        "/nope",
        Some(valid_auth_header()),
        Some(host_manifest_encoded()),
    )
    .await;
    assert_eq!(unknown.status, 404);
}

/// Missing, malformed, and wrong bearer credentials are rejected with a
/// typed UNAUTHENTICATED body before any logic runs; the correct token
/// passes everywhere.
#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_or_missing_auth() {
    let (addr, _state) = spawn_host().await;

    for (path, authorization) in [
        ("/system/health", None),
        ("/system/health", Some("Bearer not-the-token")),
        ("/system/health", Some(TEST_TOKEN)),
        ("/workspaces", None),
        ("/tasks", Some("Bearer not-the-token")),
    ] {
        let rejected = get(addr, path, authorization, Some(host_manifest_encoded())).await;
        assert_eq!(rejected.status, 401, "{path} must reject {authorization:?}");
        let body = rejected.body_json();
        assert_eq!(body["code"], "UNAUTHENTICATED");
        let message = body["message"].as_str().expect("string message");
        assert!(!message.contains(TEST_TOKEN), "must never echo the token");
    }

    // The valid token gets through everywhere.
    for path in ["/system/health", "/system/info", "/workspaces", "/tasks"] {
        let ok = get_authed(addr, path).await;
        assert_eq!(ok.status, 200, "{path} accepts the valid token");
    }
}

/// Unknown routes never answer an unauthenticated caller (a bare 404 would
/// be a discovery oracle): missing and wrong credentials get the same typed
/// UNAUTHENTICATED rejection with a constant body that never echoes the
/// token. A valid caller probing an unknown path gets the router's plain
/// 404 with no method manifest demanded of it.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_routes_require_auth_but_not_a_manifest() {
    let (addr, _state) = spawn_host().await;

    for authorization in [None, Some("Bearer not-the-token"), Some(TEST_TOKEN)] {
        let rejected = get(addr, "/nope", authorization, None).await;
        assert_eq!(rejected.status, 401, "{authorization:?} must be rejected");
        let body = rejected.body_json();
        assert_eq!(body["code"], "UNAUTHENTICATED");
        assert_eq!(
            body["message"], "missing or invalid local token",
            "the rejection body stays constant"
        );
        let message = body["message"].as_str().expect("string message");
        assert!(!message.contains(TEST_TOKEN), "must never echo the token");
    }

    // Authenticated but manifest-less: still a plain 404, not
    // INVALID_ARGUMENT, with no typed gate code in the body.
    let not_found = get(addr, "/nope", Some(valid_auth_header()), None).await;
    assert_eq!(not_found.status, 404);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&not_found.body).is_err(),
        "an unknown route answers plain, without a typed gate body"
    );
}

/// The per-method manifest is mandatory on every endpoint: missing or
/// malformed manifests are typed INVALID_ARGUMENT (400).
#[tokio::test(flavor = "multi_thread")]
async fn requires_a_negotiable_request_manifest() {
    let (addr, _state) = spawn_host().await;

    // Authenticated but no manifest header at all.
    let rejected = get(addr, "/system/health", Some(valid_auth_header()), None).await;
    assert_eq!(rejected.status, 400);
    assert_eq!(rejected.body_json()["code"], "INVALID_ARGUMENT");

    // Malformed manifest value.
    let rejected = get(
        addr,
        "/system/health",
        Some(valid_auth_header()),
        Some("v1:not-an-entry"),
    )
    .await;
    assert_eq!(rejected.status, 400);
    assert_eq!(rejected.body_json()["code"], "INVALID_ARGUMENT");
}

/// A required method missing from an otherwise valid manifest fails typed
/// INCOMPATIBLE_METHOD_MANIFEST (412), naming exactly the missing method.
#[tokio::test(flavor = "multi_thread")]
async fn rejects_required_missing_method_naming_only_the_offender() {
    let (addr, _state) = spawn_host().await;

    let without_task_list: Vec<_> = full_floor_entries()
        .into_iter()
        .filter(|(name, _, _)| name != "task.list")
        .collect();
    let rejected = get(
        addr,
        "/system/health",
        Some(valid_auth_header()),
        Some(&peer_manifest(&without_task_list)),
    )
    .await;
    assert_eq!(rejected.status, 412);
    let body = rejected.body_json();
    assert_eq!(body["code"], "INCOMPATIBLE_METHOD_MANIFEST");
    let message = body["message"].as_str().expect("string message");
    assert!(message.contains("task.list"));
    assert!(!message.contains("system.health"));
}

/// A major mismatch on one required method fails without implicating any
/// compatible method.
#[tokio::test(flavor = "multi_thread")]
async fn rejects_major_mismatch_naming_only_the_offender() {
    let (addr, _state) = spawn_host().await;

    let entries: Vec<(String, u32, u32)> = full_floor_entries()
        .into_iter()
        .map(|(name, major, minor)| {
            if name == "workspace.list" {
                (name, 2, minor)
            } else {
                (name, major, minor)
            }
        })
        .collect();
    let rejected = get(
        addr,
        "/workspaces",
        Some(valid_auth_header()),
        Some(&peer_manifest(&entries)),
    )
    .await;
    assert_eq!(rejected.status, 412);
    let body = rejected.body_json();
    assert_eq!(body["code"], "INCOMPATIBLE_METHOD_MANIFEST");
    let message = body["message"].as_str().expect("string message");
    assert!(message.contains("workspace.list"));
    assert!(!message.contains("system.health"));
}

/// Peer-only extras in the manifest are ignored: an otherwise compatible
/// peer keeps calling every Host endpoint even if it advertises methods this
/// Host has never heard of.
#[tokio::test(flavor = "multi_thread")]
async fn extra_peer_methods_do_not_block_unrelated_endpoints() {
    let (addr, _state) = spawn_host().await;

    let mut entries = full_floor_entries();
    entries.push(("future.clientOnly".to_owned(), 9, 9));
    let manifest = peer_manifest(&entries);

    let page = get(addr, "/tasks", Some(valid_auth_header()), Some(&manifest)).await;
    assert_eq!(page.status, 200);
    assert_advertises_host_manifest(&page);
    assert_eq!(page.body_json()["tasks"], serde_json::json!([]));

    let info = get(
        addr,
        "/system/info",
        Some(valid_auth_header()),
        Some(&manifest),
    )
    .await;
    assert_eq!(info.status, 200);
    assert_advertises_host_manifest(&info);
}

/// A peer advertising `task.list` at the bridged older minor 1.0
/// interoperates end to end, and the declared bridge actually executes: the
/// v1.1+ additive `servedAtUnixMs` field is stripped from the response. The
/// same endpoint keeps serving the full shape to current-minor peers.
#[tokio::test(flavor = "multi_thread")]
async fn bridged_older_minor_interoperates_through_executed_adaptation() {
    let (addr, _state) = spawn_host().await;

    let entries: Vec<(String, u32, u32)> = full_floor_entries()
        .into_iter()
        .map(|(name, major, minor)| {
            if name == "task.list" {
                (name, major, 0)
            } else {
                (name, major, minor)
            }
        })
        .collect();
    let bridged = get(
        addr,
        "/tasks",
        Some(valid_auth_header()),
        Some(&peer_manifest(&entries)),
    )
    .await;
    assert_eq!(bridged.status, 200);
    assert_advertises_host_manifest(&bridged);
    let body = bridged.body_json();
    assert_eq!(body["tasks"], serde_json::json!([]));
    assert!(
        body.get("servedAtUnixMs").is_none(),
        "the declared 1.0 bridge must strip the additive field: {body}"
    );

    // A current-minor peer receives the richer response.
    let current = get_authed(addr, "/tasks").await;
    assert_eq!(current.status, 200);
    assert!(
        current.body_json().get("servedAtUnixMs").is_some(),
        "current peers keep the v1.1+ page timestamp"
    );
}

/// Minor 1 of `task.list` was never published and has no declared bridge:
/// a numerically plausible peer advertising it is refused with typed 412,
/// proving negotiation needs executable adapters, not just numbers.
#[tokio::test(flavor = "multi_thread")]
async fn undeclared_older_minor_is_refused_with_412() {
    let (addr, _state) = spawn_host().await;

    let entries: Vec<(String, u32, u32)> = full_floor_entries()
        .into_iter()
        .map(|(name, major, minor)| {
            if name == "task.list" {
                (name, major, 1)
            } else {
                (name, major, minor)
            }
        })
        .collect();
    let rejected = get(
        addr,
        "/tasks",
        Some(valid_auth_header()),
        Some(&peer_manifest(&entries)),
    )
    .await;
    assert_eq!(
        rejected.status, 412,
        "an undeclared older minor must fail with 412"
    );
    let body = rejected.body_json();
    assert_eq!(body["code"], "INCOMPATIBLE_METHOD_MANIFEST");
    let message = body["message"].as_str().expect("string message");
    assert!(message.contains("task.list"));
}

/// An open SSE subscription: parsed response head plus the raw stream so
/// frames can be read incrementally (the stream never ends on its own).
struct SseSubscription {
    status: u16,
    headers: Vec<(String, String)>,
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl SseSubscription {
    fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Reads the next `data:` frame as JSON, failing the test if it does not
    /// arrive within [`READ_TIMEOUT`].
    async fn next_frame(&mut self) -> serde_json::Value {
        loop {
            if let Some(value) = self.pop_buffered_frame() {
                return value;
            }
            let mut chunk = [0u8; 1024];
            let read = tokio::time::timeout(READ_TIMEOUT, self.stream.read(&mut chunk))
                .await
                .expect("frame arrives in time")
                .expect("stream stays readable");
            assert!(read > 0, "stream closed before the next frame");
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    fn pop_buffered_frame(&mut self) -> Option<serde_json::Value> {
        let offset = self
            .buffered
            .windows(5)
            .position(|window| window == b"data:")?;
        let line_start = offset + 5;
        let line_end_rel = self.buffered[line_start..]
            .iter()
            .position(|b| *b == b'\n')?;
        let line_end = line_start + line_end_rel;
        let data: String = std::str::from_utf8(&self.buffered[line_start..line_end])
            .expect("SSE frame is UTF-8")
            .trim()
            .to_owned();
        self.buffered.drain(..line_end + 1);
        Some(serde_json::from_str(&data).expect("decodable SSE frame"))
    }

    /// Waits for the server to close the stream, collecting any trailing
    /// data frames first.
    async fn drain_frames_until_eof(mut self) -> Vec<serde_json::Value> {
        let mut frames = Vec::new();
        loop {
            if let Some(value) = self.pop_buffered_frame() {
                frames.push(value);
                continue;
            }
            let mut chunk = [0u8; 256];
            let read = tokio::time::timeout(READ_TIMEOUT, self.stream.read(&mut chunk))
                .await
                .expect("stream closes in time")
                .expect("readable until close");
            if read == 0 {
                return frames;
            }
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }
}

/// Opens an authenticated SSE subscription with `Connection: close`, so a
/// finished response is followed by a socket close the client can detect.
async fn open_sse(addr: SocketAddr, last_outage_id: Option<&str>) -> SseSubscription {
    open_sse_with_deadline(addr, last_outage_id, None).await
}

async fn open_sse_with_deadline(
    addr: SocketAddr,
    last_outage_id: Option<&str>,
    deadline: Option<u64>,
) -> SseSubscription {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to host for SSE");
    let mut request =
        format!("GET /system/events HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    request.push_str(&format!(
        "{}: {}\r\n",
        auth::AUTH_METADATA_KEY,
        valid_auth_header()
    ));
    request.push_str(&format!(
        "{MANIFEST_METADATA_KEY}: {}\r\n",
        host_manifest_encoded()
    ));
    if let Some(last_outage_id) = last_outage_id {
        request.push_str(&format!("{LAST_OUTAGE_HEADER}: {last_outage_id}\r\n"));
    }
    if let Some(deadline) = deadline {
        request.push_str(&format!("x-lazarus-deadline: {deadline}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write SSE request");

    // Read just the response head.
    let mut buffered = Vec::new();
    let head_end = loop {
        if let Some(position) = buffered.windows(4).position(|w| w == b"\r\n\r\n") {
            break position;
        }
        let mut chunk = [0u8; 1024];
        let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .expect("response head arrives in time")
            .expect("head is readable");
        assert!(read > 0, "connection closed before response head");
        buffered.extend_from_slice(&chunk[..read]);
    };

    let head = std::str::from_utf8(&buffered[..head_end]).expect("head is ASCII");
    let status: u16 = head
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status code");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    SseSubscription {
        status,
        headers,
        stream,
        buffered: buffered[head_end + 4..].to_vec(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_sse_closes_at_the_caller_deadline() {
    let (addr, _state) = spawn_host().await;
    let deadline = protocol_rs::deadline::unix_now_ms() + 100;
    let mut sse = open_sse_with_deadline(addr, None, Some(deadline)).await;

    assert_eq!(frame_type(&sse.next_frame().await), "outage");
    assert_eq!(frame_type(&sse.next_frame().await), "snapshot");
    assert!(
        sse.drain_frames_until_eof().await.is_empty(),
        "an idle stream closes without fabricating frames"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn graceful_shutdown_closes_a_deadline_free_sse_stream() {
    let (addr, state) = spawn_host().await;
    let mut sse = open_sse(addr, None).await;
    assert_eq!(frame_type(&sse.next_frame().await), "outage");
    assert_eq!(frame_type(&sse.next_frame().await), "snapshot");

    state.begin_shutdown();
    assert!(
        sse.drain_frames_until_eof().await.is_empty(),
        "shutdown closes the idle stream without fabricating frames"
    );
}

fn frame_type(frame: &serde_json::Value) -> &str {
    frame["type"].as_str().expect("typed frame")
}

/// A fresh subscription always opens with the outage tombstone and an
/// authoritative snapshot before anything live.
#[tokio::test(flavor = "multi_thread")]
async fn sse_first_subscription_sends_tombstone_then_snapshot() {
    let (addr, state) = spawn_host().await;

    let mut sse = open_sse(addr, None).await;
    assert_eq!(sse.status, 200);

    let tombstone = sse.next_frame().await;
    assert_eq!(frame_type(&tombstone), "outage");
    let outage_id = tombstone["outageId"].as_str().expect("outage id");
    assert!(!outage_id.is_empty());
    assert_eq!(outage_id, state.bus.outage_id());

    let snapshot = sse.next_frame().await;
    assert_eq!(frame_type(&snapshot), "snapshot");
    assert_eq!(
        snapshot,
        serde_json::json!({"type": "snapshot", "workspaces": [], "tasks": []})
    );

    // The SSE success response advertises this Host's complete manifest.
    assert_advertises_host_manifest_by_name(&sse);
}

fn assert_advertises_host_manifest_by_name(sse: &SseSubscription) {
    let advertised: MethodManifest = sse
        .header(MANIFEST_METADATA_KEY)
        .unwrap_or_else(|| panic!("SSE response advertises the host manifest"))
        .parse()
        .expect("decodable manifest");
    assert_eq!(advertised, host_manifest());
}

/// Reconnecting mid-outage suppresses the tombstone but never the snapshot.
#[tokio::test(flavor = "multi_thread")]
async fn sse_same_outage_dedupes_tombstone_but_still_snapshots() {
    let (addr, _state) = spawn_host().await;

    let mut first = open_sse(addr, None).await;
    let tombstone = first.next_frame().await;
    let outage_id = tombstone["outageId"]
        .as_str()
        .expect("outage id")
        .to_owned();

    let mut reconnect = open_sse(addr, Some(&outage_id)).await;
    let only_frame = reconnect.next_frame().await;
    assert_eq!(frame_type(&only_frame), "snapshot");

    // An unknown or stale id still gets the current tombstone exactly once.
    let mut stale = open_sse(addr, Some("outage-from-a-past-life")).await;
    let tombstone = stale.next_frame().await;
    assert_eq!(frame_type(&tombstone), "outage");
    assert_eq!(tombstone["outageId"], outage_id);
    let snapshot = stale.next_frame().await;
    assert_eq!(frame_type(&snapshot), "snapshot");
}

/// Each Host incarnation mints a new stable outage id, and a client that
/// reports the previous one still receives the fresh tombstone.
#[tokio::test(flavor = "multi_thread")]
async fn sse_new_host_outage_produces_new_tombstone() {
    let (addr_a, _state_a) = spawn_host().await;
    let (addr_b, state_b) = spawn_host().await;

    let mut sub_a = open_sse(addr_a, None).await;
    let tombstone_a = sub_a.next_frame().await;
    let outage_a = tombstone_a["outageId"]
        .as_str()
        .expect("outage id")
        .to_owned();

    assert_ne!(state_b.bus.outage_id(), outage_a, "restart mints a new id");

    let mut sub_b = open_sse(addr_b, Some(&outage_a)).await;
    let tombstone_b = sub_b.next_frame().await;
    assert_eq!(frame_type(&tombstone_b), "outage");
    assert_eq!(tombstone_b["outageId"], state_b.bus.outage_id());
}

/// Live sequenced events flow after the opening snapshot.
#[tokio::test(flavor = "multi_thread")]
async fn sse_live_event_follows_snapshot() {
    let (addr, state) = spawn_host().await;

    let mut sse = open_sse(addr, None).await;
    assert_eq!(frame_type(&sse.next_frame().await), "outage");
    assert_eq!(frame_type(&sse.next_frame().await), "snapshot");

    let published = state.bus.publish();
    let live = sse.next_frame().await;
    assert_eq!(live, serde_json::json!({"type": "live", "sequence": 1}));
    assert_eq!(published, lazarus_hostd::EventFrame::Live { sequence: 1 });
}

/// Events published before a client subscribes are never replayed: the
/// broadcast feed only queues frames published from the subscription onward,
/// so nothing older leaks out ahead of newer live frames.
#[tokio::test(flavor = "multi_thread")]
async fn sse_does_not_replay_pre_subscription_events_as_live_frames() {
    let (addr, state) = spawn_host().await;

    // History that predates the subscription entirely.
    for _ in 0..3 {
        state.bus.publish();
    }

    let mut sse = open_sse(addr, None).await;
    assert_eq!(frame_type(&sse.next_frame().await), "outage");
    assert_eq!(frame_type(&sse.next_frame().await), "snapshot");

    // A publish after the subscription is delivered exactly once, and
    // nothing older leaks out first.
    state.bus.publish();
    let live = sse.next_frame().await;
    assert_eq!(live, serde_json::json!({"type": "live", "sequence": 4}));
}

/// A subscriber that falls behind is disconnected rather than served skipped
/// frames; it must resubscribe for a fresh snapshot.
///
/// Pinned to a current-thread runtime so the overflow burst below cannot be
/// drained concurrently by the server task, making the lag deterministic.
#[tokio::test(flavor = "current_thread")]
async fn sse_lag_closes_the_stream() {
    let (addr, state) = spawn_host().await;

    let mut sse = open_sse(addr, None).await;
    assert_eq!(frame_type(&sse.next_frame().await), "outage");
    assert_eq!(frame_type(&sse.next_frame().await), "snapshot");

    // Overflow the broadcast buffer without reading any live frames.
    for _ in 0..=64 {
        state.bus.publish();
    }

    // Whatever was delivered, the stream ends instead of continuing.
    let frames = sse.drain_frames_until_eof().await;
    assert!(
        frames.len() <= 66,
        "no fabricated frames beyond what was published"
    );
}

/// `/system/events` sits behind the same transport gate as every unary
/// endpoint: auth and a negotiable manifest are both mandatory.
#[tokio::test(flavor = "multi_thread")]
async fn sse_requires_auth_and_manifest() {
    let (addr, _state) = spawn_host().await;

    let rejected = get(addr, "/system/events", None, Some(host_manifest_encoded())).await;
    assert_eq!(rejected.status, 401);
    assert_eq!(rejected.body_json()["code"], "UNAUTHENTICATED");

    let rejected = get(addr, "/system/events", Some(valid_auth_header()), None).await;
    assert_eq!(rejected.status, 400);
    assert_eq!(rejected.body_json()["code"], "INVALID_ARGUMENT");

    // And the method participates in per-method negotiation.
    let without_events: Vec<_> = full_floor_entries()
        .into_iter()
        .filter(|(name, _, _)| name != "system.subscribeEvents")
        .collect();
    let rejected = get(
        addr,
        "/system/events",
        Some(valid_auth_header()),
        Some(&peer_manifest(&without_events)),
    )
    .await;
    assert_eq!(rejected.status, 412);
    assert!(
        rejected.body_json()["message"]
            .as_str()
            .expect("message")
            .contains("system.subscribeEvents")
    );
}

/// The cancellation/deadline contract is enforced operationally end to end:
/// an already-elapsed deadline is a typed 504 DEADLINE_EXCEEDED before any
/// handler runs, a malformed header is a typed 400, and a healthy future
/// deadline passes straight through.
#[tokio::test(flavor = "multi_thread")]
async fn deadlines_are_enforced_with_typed_canonical_errors() {
    let (addr, _state) = spawn_host().await;
    let now = protocol_rs::deadline::unix_now_ms();

    // Elapsed budget: immediate typed DEADLINE_EXCEEDED (and the canonical
    // envelope marks it retryable).
    let elapsed = get_with_extras(
        addr,
        "/tasks",
        Some(valid_auth_header()),
        Some(host_manifest_encoded()),
        &[(
            protocol_rs::deadline::DEADLINE_HEADER,
            (now - 1_000).to_string(),
        )],
    )
    .await;
    assert_eq!(elapsed.status, 504);
    let body = wire::decode_protocol_error(&elapsed.body_json()).expect("canonical envelope");
    assert_eq!(body.code.as_str(), "DEADLINE_EXCEEDED");
    assert!(body.retryable);

    // Malformed value: typed INVALID_ARGUMENT, never silently ignored.
    let malformed = get_with_extras(
        addr,
        "/tasks",
        Some(valid_auth_header()),
        Some(host_manifest_encoded()),
        &[(protocol_rs::deadline::DEADLINE_HEADER, "soon".to_string())],
    )
    .await;
    assert_eq!(malformed.status, 400);
    assert_eq!(malformed.body_json()["code"], "INVALID_ARGUMENT");

    // A healthy deadline within the shared client budget serves normally.
    let healthy = get_with_extras(
        addr,
        "/tasks",
        Some(valid_auth_header()),
        Some(host_manifest_encoded()),
        &[(
            protocol_rs::deadline::DEADLINE_HEADER,
            Deadline::header_from_budget(now, DEFAULT_RPC_BUDGET_MS),
        )],
    )
    .await;
    assert_eq!(healthy.status, 200);
    assert_advertises_host_manifest(&healthy);
}

/// Unknown additive fields are tolerated at the boundary: a live Host
/// response carrying an extra v-next field still decodes through the
/// generated bindings of a current client.
#[tokio::test(flavor = "multi_thread")]
async fn live_responses_tolerate_unknown_additive_fields_end_to_end() {
    let (addr, _state) = spawn_host().await;

    let tasks = get_authed(addr, "/tasks").await;
    assert_eq!(tasks.status, 200);
    let mut body = tasks.body_json();
    body["futureAdditiveField"] = serde_json::json!({"anything": true});
    let decoded = wire::decode_task_list_response(&body).expect("additive tolerated");
    assert!(decoded.tasks.is_empty());

    // The same holds for the error path: gate rejections decode through the
    // generated error envelope with canonical retryability.
    let rejected = get(addr, "/system/health", None, None).await;
    assert_eq!(rejected.status, 401);
    let error = wire::decode_protocol_error(&rejected.body_json()).expect("canonical");
    assert_eq!(error.code.as_str(), "UNAUTHENTICATED");
    assert!(!error.retryable);
}

/// In-contract query parameters are accepted; out-of-contract ones are
/// rejected by the generated request validation before handlers run.
#[tokio::test(flavor = "multi_thread")]
async fn task_list_query_binds_to_the_generated_request_contract() {
    let (addr, _state) = spawn_host().await;

    let ok = get_authed(addr, "/tasks?pageSize=10&cursor=abc").await;
    assert_eq!(ok.status, 200);

    let too_large = get_authed(addr, "/tasks?pageSize=101").await;
    assert_eq!(too_large.status, 400);
    assert_eq!(too_large.body_json()["code"], "INVALID_ARGUMENT");

    let zero = get_authed(addr, "/tasks?pageSize=0").await;
    assert_eq!(zero.status, 400);
    assert_eq!(zero.body_json()["code"], "INVALID_ARGUMENT");

    let wrong_type = get_authed(addr, "/tasks?pageSize=abc").await;
    assert_eq!(wrong_type.status, 400);
    assert_eq!(wrong_type.body_json()["code"], "INVALID_ARGUMENT");
}
