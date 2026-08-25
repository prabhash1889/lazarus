//! End-to-end Phase 3.2 transport checks: the authenticated Lazarus
//! Protocol surface answered over the local IPC endpoint (Windows named
//! pipe or Unix domain socket), including typed auth failures, the SSE
//! event subscription ending on Host shutdown, concurrent clients, and
//! single-owner binding.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lazarus_hostd::ipc::{self, IpcEndpoint};
use lazarus_hostd::{HostServices, HostState};
use protocol_rs::auth::{self, bearer_header};
use protocol_rs::manifest::{MANIFEST_METADATA_KEY, host_manifest_encoded};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_TOKEN: &str = "ipc-integration-token";
const IO_TIMEOUT: Duration = Duration::from_secs(10);

static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(1);

fn unique_endpoint(tag: &str) -> IpcEndpoint {
    let id = NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed);
    #[cfg(windows)]
    {
        let _ = tag;
        IpcEndpoint::NamedPipe(format!(
            r"\\.\pipe\lazarus-hostd-it-{tag}-{}-{id}",
            std::process::id()
        ))
    }
    #[cfg(not(windows))]
    {
        static NEXT_DIR: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "lazarus-hostd-ipc-it-{tag}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        IpcEndpoint::UnixSocket(dir.join(format!("hostd-{id}.sock")))
    }
}

async fn spawn_ipc_host(endpoint: &IpcEndpoint) -> Arc<HostState> {
    let state = Arc::new(HostState::with_event_capacity(64));
    let services = HostServices::new(state.clone(), Arc::from(TEST_TOKEN));
    let app = lazarus_hostd::build_router(services);
    let listener = ipc::IpcListener::bind(endpoint).expect("bind the IPC endpoint");
    tokio::spawn(ipc::serve_ipc(listener, app, state.subscribe_shutdown()));
    state
}

/// A platform-local duplex stream connected to the IPC endpoint, retrying
/// briefly while the accept loop spins up.
async fn dial(endpoint: &IpcEndpoint) -> io::Result<IpcStream> {
    let mut last = None;
    for _ in 0..50u32 {
        match connect_once(endpoint) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err(last.expect("at least one dial attempt"))
}

#[cfg(windows)]
type IpcStream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(windows)]
fn connect_once(endpoint: &IpcEndpoint) -> io::Result<IpcStream> {
    let name = match endpoint {
        IpcEndpoint::NamedPipe(name) => name,
        IpcEndpoint::UnixSocket(_) => panic!("windows tests use named pipes"),
    };
    tokio::net::windows::named_pipe::ClientOptions::new().open(name)
}

#[cfg(not(windows))]
type IpcStream = tokio::net::UnixStream;

#[cfg(not(windows))]
fn connect_once(endpoint: &IpcEndpoint) -> io::Result<IpcStream> {
    let path = match endpoint {
        IpcEndpoint::UnixSocket(path) => path,
        IpcEndpoint::NamedPipe(_) => panic!("unix tests use domain sockets"),
    };
    tokio::net::UnixStream::connect(path)
}

/// Transient local-transport failures worth redialing: a Windows client can
/// win the race against the server's pending `connect()`, surfacing as
/// broken-pipe style errors on first use.
fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
    )
}

struct RawResponse {
    head: String,
    body: Vec<u8>,
}

impl RawResponse {
    fn status(&self) -> u16 {
        self.head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("status line in {:?}", self.head))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("JSON body")
    }
}

fn unary_request(path: &str, token: Option<&str>) -> String {
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n");
    if let Some(token) = token {
        request.push_str(&format!(
            "{}: {}\r\n",
            auth::AUTH_METADATA_KEY,
            bearer_header(token)
        ));
    }
    request.push_str(&format!(
        "{MANIFEST_METADATA_KEY}: {}\r\n",
        host_manifest_encoded()
    ));
    request.push_str("Connection: close\r\n\r\n");
    request
}

/// One dial + request + full-response read. Fails with the transport error
/// so callers decide whether to retry.
async fn try_exchange(endpoint: &IpcEndpoint, request: &str) -> io::Result<RawResponse> {
    let mut stream = dial(endpoint).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    // `Connection: close` makes EOF the unambiguous response end.
    tokio::time::timeout(IO_TIMEOUT, stream.read_to_end(&mut raw)).await??;
    Ok(split_response(raw))
}

/// Retries transient transport races, then asserts the exchange completed.
async fn exchange(endpoint: &IpcEndpoint, request: &str) -> RawResponse {
    let mut last = None;
    for _ in 0..10u32 {
        match try_exchange(endpoint, request).await {
            Ok(response) => return response,
            Err(error) => {
                assert!(is_transient(&error), "non-transient failure: {error}");
                last = Some(error.to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("exchange kept failing transiently: {:?}", last);
}

fn split_response(raw: Vec<u8>) -> RawResponse {
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header terminator");
    RawResponse {
        head: String::from_utf8_lossy(&raw[..head_end]).into_owned(),
        body: raw[head_end + 4..].to_vec(),
    }
}

/// Dials and sends `request`, tolerating the startup write race, returning
/// the live stream for continued reading.
async fn open_stream(endpoint: &IpcEndpoint, request: &str) -> IpcStream {
    let mut last = None;
    for _ in 0..10u32 {
        let mut stream = dial(endpoint).await.expect("dial");
        match stream.write_all(request.as_bytes()).await {
            Ok(()) => return stream,
            Err(error) => {
                assert!(is_transient(&error), "{error}");
                last = Some(error.to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("opening the stream kept failing transiently: {:?}", last);
}

/// Reads from an already-open stream until `done` matches the accumulated
/// bytes or EOF arrives.
async fn read_until(
    stream: &mut IpcStream,
    done: impl Fn(&[u8]) -> bool,
    budget: Duration,
) -> Vec<u8> {
    let mut buffer = Vec::new();
    let outcome = tokio::time::timeout(budget, async {
        loop {
            if done(&buffer) {
                return Ok::<(), io::Error>(());
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk).await {
                // EOF only counts when the predicate already matched;
                // otherwise the stream ended short.
                Ok(0) => {
                    if done(&buffer) {
                        return Ok(());
                    }
                    return Err(io::Error::other("stream ended before its predicate"));
                }
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                Err(error) => return Err(error),
            }
        }
    })
    .await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("transport error while streaming: {error}"),
        Err(_elapsed) => panic!(
            "the stream never satisfied its predicate within {budget:?}: {}",
            String::from_utf8_lossy(&buffer)
        ),
    }
    buffer
}

#[tokio::test(flavor = "multi_thread")]
async fn unary_requests_answer_over_local_ipc() {
    let endpoint = unique_endpoint("unary");
    spawn_ipc_host(&endpoint).await;

    // Authenticated info succeeds and advertises the Host manifest plus the
    // v1.1 incarnation stamp.
    let info = exchange(&endpoint, &unary_request("/system/info", Some(TEST_TOKEN))).await;
    assert_eq!(info.status(), 200);
    assert_eq!(
        info.header(MANIFEST_METADATA_KEY),
        Some(host_manifest_encoded())
    );
    assert_eq!(
        info.body_json()["hostVersion"],
        env!("CARGO_PKG_VERSION").to_string()
    );
    assert!(
        info.body_json()["startedAtUnixMs"].as_u64().unwrap_or(0) > 0,
        "the incarnation stamp is served"
    );

    // A wrong token is a typed UNAUTHENTICATED rejection, not a hang.
    let wrong_token = exchange(
        &endpoint,
        &unary_request("/system/info", Some("not-the-token")),
    )
    .await;
    assert_eq!(wrong_token.status(), 401);
    let envelope = wrong_token.body_json();
    assert_eq!(envelope["code"], "UNAUTHENTICATED");
    assert!(
        !envelope["message"]
            .as_str()
            .unwrap_or("")
            .contains(TEST_TOKEN),
        "rejections never echo the presented token"
    );

    // A missing token is refused identically.
    let no_token = exchange(&endpoint, &unary_request("/system/info", None)).await;
    assert_eq!(no_token.status(), 401);
    assert_eq!(no_token.body_json()["code"], "UNAUTHENTICATED");
}

#[tokio::test(flavor = "multi_thread")]
async fn event_subscription_streams_prefix_then_ends_on_shutdown() {
    let endpoint = unique_endpoint("events");
    let state = spawn_ipc_host(&endpoint).await;

    // Subscribe without `Connection: close`: the SSE feed stays open until
    // the Host drains or the deadline elapses.
    let mut request = String::from("GET /system/events HTTP/1.1\r\nHost: localhost\r\n");
    request.push_str(&format!(
        "{}: {}\r\n",
        auth::AUTH_METADATA_KEY,
        bearer_header(TEST_TOKEN)
    ));
    request.push_str(&format!(
        "{MANIFEST_METADATA_KEY}: {}\r\n",
        host_manifest_encoded()
    ));
    request.push_str("\r\n");

    let mut stream = open_stream(&endpoint, &request).await;

    // The opening prefix is the outage tombstone followed by the
    // authoritative snapshot.
    let buffer = read_until(
        &mut stream,
        |bytes| {
            let text = String::from_utf8_lossy(bytes);
            text.contains("\"type\":\"outage\"") && text.contains("\"type\":\"snapshot\"")
        },
        IO_TIMEOUT,
    )
    .await;
    let text = String::from_utf8_lossy(&buffer);
    let outage_pos = text.find("\"type\":\"outage\"").expect("tombstone present");
    let snapshot_pos = text
        .find("\"type\":\"snapshot\"")
        .expect("snapshot present");
    assert!(
        outage_pos < snapshot_pos,
        "the tombstone precedes the authoritative snapshot"
    );

    // Graceful Host shutdown ends every live event response promptly: the
    // chunked body terminates (the HTTP/1.1 connection itself stays open,
    // so the terminator - not EOF - is the honest end signal).
    state.begin_shutdown();
    let tail = read_until(
        &mut stream,
        |bytes| bytes.ends_with(b"0\r\n\r\n"),
        Duration::from_secs(5),
    )
    .await;
    assert!(
        tail.ends_with(b"0\r\n\r\n"),
        "the event response terminated with a complete chunked body"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_clients_are_served_independently() {
    let endpoint = unique_endpoint("concurrent");
    spawn_ipc_host(&endpoint).await;

    let first = tokio::spawn({
        let endpoint = endpoint.clone();
        async move {
            exchange(
                &endpoint,
                &unary_request("/system/health", Some(TEST_TOKEN)),
            )
            .await
        }
    });
    let second = tokio::spawn({
        let endpoint = endpoint.clone();
        async move { exchange(&endpoint, &unary_request("/system/info", Some(TEST_TOKEN))).await }
    });

    let (health, info) = tokio::join!(first, second);
    let health = health.expect("task 1");
    let info = info.expect("task 2");
    assert_eq!(health.status(), 200);
    assert_eq!(health.body_json()["status"], "SERVING");
    assert_eq!(info.status(), 200);
    assert!(info.header(MANIFEST_METADATA_KEY).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_the_same_endpoint_twice_is_refused() {
    let endpoint = unique_endpoint("ownership");
    let first = ipc::IpcListener::bind(&endpoint).expect("first bind owns the endpoint");
    let second = ipc::IpcListener::bind(&endpoint);
    assert!(
        second.is_err(),
        "a second bind must not steal or share the live endpoint"
    );
    drop(first);
}
