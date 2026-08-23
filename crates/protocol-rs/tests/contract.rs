//! Contract tests for the Lazarus Protocol wire surface (Phase 1 slice
//! P1.1). A stub host serves the generated services over loopback so client
//! behavior is exercised exactly as Desktop/CLI will experience it.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use protocol_rs::generated::{
    system_service_client::SystemServiceClient, system_service_server::SystemService,
    task_service_client::TaskServiceClient, task_service_server::TaskService, ClientHello,
    Envelope, GetInfoRequest, GetInfoResponse, HealthRequest, HealthResponse, HostReply,
    ListTasksRequest, ListTasksResponse, PaginationRequest, PaginationResponse, ProtocolVersion,
    ServingStatus, SubscribeEventsRequest, TaskStatus, TaskSummary,
};
use protocol_rs::handshake;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tokio_stream::Stream;
use tonic::transport::{Channel, Server};
use tonic::{Request, Response, Status};

type BoxEnvelopeStream = Pin<Box<dyn Stream<Item = Result<Envelope, Status>> + Send>>;

/// Records when the server-side response stream is dropped, which is how a
/// tonic server observes transport-level client cancellation.
struct CancelledOnDrop<S> {
    inner: S,
    cancelled: Arc<AtomicBool>,
}

impl<S: Stream + Unpin> Stream for CancelledOnDrop<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl<S> Drop for CancelledOnDrop<S> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// Stub host that speaks protocol 1.6: newer than this crate's 1.0, same
/// major. Stub responses only - Phase 1 does not implement real state.
#[derive(Clone, Default)]
struct StubSystem {
    stream_dropped: Arc<AtomicBool>,
}

#[tonic::async_trait]
impl SystemService for StubSystem {
    type SubscribeEventsStream = BoxEnvelopeStream;

    async fn negotiate(
        &self,
        request: Request<ClientHello>,
    ) -> Result<Response<HostReply>, Status> {
        let hello = request.into_inner();
        let host_protocol = ProtocolVersion { major: 1, minor: 6 };
        let negotiated_minor = handshake::negotiate(
            hello
                .protocol
                .as_ref()
                .unwrap_or(&ProtocolVersion::default()),
            &host_protocol,
        )?;
        Ok(Response::new(HostReply {
            host_version: "stub-host-0.1.0".into(),
            protocol: Some(host_protocol),
            negotiated_minor,
            capabilities: HashMap::from([("pty".to_string(), true)]),
        }))
    }

    async fn get_info(
        &self,
        _request: Request<GetInfoRequest>,
    ) -> Result<Response<GetInfoResponse>, Status> {
        Ok(Response::new(GetInfoResponse {
            host_version: "stub-host-0.1.0".into(),
            protocol: Some(ProtocolVersion { major: 1, minor: 6 }),
            capabilities: HashMap::from([("pty".to_string(), true)]),
        }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            status: ServingStatus::Serving as i32,
        }))
    }

    async fn subscribe_events(
        &self,
        _request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<BoxEnvelopeStream>, Status> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Envelope, Status>>(4);
        let cancelled = self.stream_dropped.clone();
        tokio::spawn(async move {
            for sequence in 0u64.. {
                let mut envelope = protocol_rs::new_envelope(format!("m-{sequence}"));
                envelope.sequence = Some(sequence);
                if tx.send(Ok(envelope)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let stream = CancelledOnDrop {
            inner: ReceiverStream::new(rx),
            cancelled,
        };
        Ok(Response::new(Box::pin(stream)))
    }
}

const TOTAL_TASKS: usize = 25;

#[derive(Default)]
struct StubTasks;

#[tonic::async_trait]
impl TaskService for StubTasks {
    async fn list(
        &self,
        request: Request<ListTasksRequest>,
    ) -> Result<Response<ListTasksResponse>, Status> {
        let pagination_req = request.into_inner().pagination.unwrap_or_default();
        let page_size = match pagination_req.page_size {
            0 => 10,
            n => n.min(50) as usize,
        };
        let offset = pagination_req
            .page_token
            .and_then(|token| token.parse::<usize>().ok())
            .unwrap_or(0);
        if offset > TOTAL_TASKS {
            return Err(Status::invalid_argument("page token out of range"));
        }

        let end = (offset + page_size).min(TOTAL_TASKS);
        let tasks = (offset..end)
            .map(|i| TaskSummary {
                id: format!("task-{i:04}"),
                title: format!("Task {i}"),
                status: TaskStatus::Pending as i32,
            })
            .collect();

        Ok(Response::new(ListTasksResponse {
            tasks,
            pagination: Some(PaginationResponse {
                next_page_token: (end < TOTAL_TASKS).then(|| end.to_string()),
            }),
        }))
    }
}

struct TestHost {
    system: SystemServiceClient<Channel>,
    tasks: TaskServiceClient<Channel>,
    stream_dropped: Arc<AtomicBool>,
}

async fn spawn_host() -> TestHost {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let stub = StubSystem::default();
    let stream_dropped = stub.stream_dropped.clone();
    tokio::spawn(async move {
        Server::builder()
            .add_service(
                protocol_rs::generated::system_service_server::SystemServiceServer::new(stub),
            )
            .add_service(
                protocol_rs::generated::task_service_server::TaskServiceServer::new(StubTasks),
            )
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .expect("stub host serves");
    });

    let endpoint = format!("http://{addr}");
    TestHost {
        system: SystemServiceClient::connect(endpoint.clone())
            .await
            .expect("connect"),
        tasks: TaskServiceClient::connect(endpoint).await.expect("connect"),
        stream_dropped,
    }
}

fn hello(major: i32, minor: i32) -> ClientHello {
    ClientHello {
        client: "contract-test".into(),
        client_version: "0.1.0".into(),
        protocol: Some(ProtocolVersion { major, minor }),
        supported_features: vec!["agent_stream_v2".into()],
        auth: None,
    }
}

#[tokio::test]
async fn older_minor_client_negotiates_with_newer_host() {
    let mut host = spawn_host().await;

    let reply = host
        .system
        .negotiate(hello(1, 4))
        .await
        .expect("compatible")
        .into_inner();
    assert_eq!(reply.protocol.map(|p| p.major), Some(1));
    assert_eq!(reply.protocol.map(|p| p.minor), Some(6));
    // Both sides fall back to the lower minor within the shared major.
    assert_eq!(reply.negotiated_minor, 4);
    assert_eq!(reply.capabilities.get("pty"), Some(&true));

    // The reverse pairing negotiates identically.
    let reply = host
        .system
        .negotiate(hello(1, 9))
        .await
        .expect("compatible")
        .into_inner();
    assert_eq!(reply.negotiated_minor, 6);
}

#[tokio::test]
async fn unsupported_major_fails_clearly() {
    let mut host = spawn_host().await;

    let err = host
        .system
        .negotiate(hello(2, 0))
        .await
        .expect_err("major mismatch must fail");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(
        err.message().contains("unsupported protocol version"),
        "failure must be self-explanatory: {}",
        err.message()
    );
    let decoded = handshake::error_from_status(&err).expect("structured error details");
    assert_eq!(
        decoded.code(),
        protocol_rs::generated::ErrorCode::UnsupportedProtocolVersion
    );
}

#[tokio::test]
async fn unary_rpcs_return_stub_responses() {
    let mut host = spawn_host().await;

    host.system
        .negotiate(hello(1, 4))
        .await
        .expect("negotiated");
    let info = host
        .system
        .get_info(GetInfoRequest {})
        .await
        .expect("info")
        .into_inner();
    assert_eq!(info.host_version, "stub-host-0.1.0");

    let health = host
        .system
        .health(HealthRequest {})
        .await
        .expect("health")
        .into_inner();
    assert_eq!(health.status(), ServingStatus::Serving);
}

#[tokio::test]
async fn task_list_paginates_to_exhaustion_without_overlap() {
    let mut host = spawn_host().await;

    let mut seen = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let reply = host
            .tasks
            .list(ListTasksRequest {
                pagination: Some(PaginationRequest {
                    page_size: 10,
                    page_token: page_token.take(),
                }),
            })
            .await
            .expect("list page")
            .into_inner();
        seen.extend(reply.tasks.into_iter().map(|t| t.id));
        match reply.pagination.and_then(|p| p.next_page_token) {
            Some(next) => page_token = Some(next),
            None => break,
        }
    }

    assert_eq!(seen.len(), TOTAL_TASKS);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "pages must not overlap or skip");

    // Out-of-range tokens are rejected explicitly rather than silently
    // returning an empty page.
    let err = host
        .tasks
        .list(ListTasksRequest {
            pagination: Some(PaginationRequest {
                page_size: 10,
                page_token: Some("9999".into()),
            }),
        })
        .await
        .expect_err("out-of-range token");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

#[tokio::test]
async fn dropping_streaming_rpc_cancels_on_the_server() {
    let mut host = spawn_host().await;
    let flag = host.stream_dropped.clone();

    let mut stream = host
        .system
        .subscribe_events(SubscribeEventsRequest { reconnect: None })
        .await
        .expect("subscribe")
        .into_inner();

    let first = tokio::time::timeout(Duration::from_secs(5), stream.message())
        .await
        .expect("first frame arrives in time")
        .expect("stream ok")
        .expect("frame present");
    assert_eq!(first.sequence, Some(0));
    assert!(!flag.load(Ordering::SeqCst), "stream still open on server");

    // Client cancels by dropping the streaming call; tonic propagates the
    // cancellation to the server by dropping its response stream. No cancel
    // RPC exists in the schema and none is needed.
    drop(stream);

    tokio::time::timeout(Duration::from_secs(10), async {
        while !flag.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("server must observe transport-level cancellation");

    // And the connection remains usable afterwards.
    host.system
        .health(HealthRequest {})
        .await
        .expect("health after cancel");
}

#[test]
fn reconnect_token_contract_roundtrips_and_expires() {
    use prost::Message;
    use std::time::{Duration, SystemTime};

    let token = protocol_rs::envelope::ReconnectToken {
        token: "reconnect-abc".into(),
        stream_id: "events".into(),
        last_sequence: 42,
        expires_at: Some(prost_types::Timestamp::from(SystemTime::now())),
    };
    let decoded = protocol_rs::envelope::ReconnectToken::decode(token.encode_to_vec().as_slice())
        .expect("decode");
    assert_eq!(decoded.token, "reconnect-abc");
    assert_eq!(decoded.last_sequence, 42);

    // A token issued in the past is unusable; a fresh one resumes from the
    // acknowledged sequence.
    assert!(!decoded.usable_at(SystemTime::now() + Duration::from_secs(1)));
}
