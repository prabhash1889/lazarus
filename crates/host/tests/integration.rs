//! End-to-end Phase 1 contract check against an in-process Host server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use lazarus_hostd::{HostServices, HostState};
use prost_types::Timestamp;
use protocol_rs::envelope::{ProtocolVersion, ReconnectToken};
use protocol_rs::error::ErrorCode;
use protocol_rs::generated::{
    HealthRequest, ListTasksRequest, ListWorkspacesRequest, SubscribeEventsRequest,
    system_service_client::SystemServiceClient, task_service_client::TaskServiceClient,
    workspace_service_client::WorkspaceServiceClient,
};
use protocol_rs::handshake::{ClientHello, error_from_status};
use protocol_rs::{
    SystemServiceServer, TaskServiceServer, WorkspaceServiceServer, generated::ServingStatus,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Endpoint, Server};
use tonic::{Code, Request};

async fn spawn_host() -> (SocketAddr, Arc<HostState>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let state = Arc::new(HostState::with_event_capacity(64));
    let services = HostServices::new(state.clone());
    tokio::spawn(async move {
        Server::builder()
            .add_service(SystemServiceServer::new(services.clone()))
            .add_service(WorkspaceServiceServer::new(services.clone()))
            .add_service(TaskServiceServer::new(services))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .expect("host server runs");
    });
    (addr, state)
}

fn hello(major: i32, minor: i32) -> ClientHello {
    ClientHello {
        client: "integration-test".to_owned(),
        client_version: "0.1.0".to_owned(),
        protocol: Some(ProtocolVersion { major, minor }),
        supported_features: vec!["events".to_owned()],
        auth: None,
    }
}

fn reconnect_token(
    stream_id: String,
    last_sequence: u64,
    expires_in: Duration,
) -> SubscribeEventsRequest {
    SubscribeEventsRequest {
        reconnect: Some(ReconnectToken {
            token: "t".to_owned(),
            stream_id,
            last_sequence,
            expires_at: Some(Timestamp::from(SystemTime::now() + expires_in)),
        }),
    }
}

async fn next_envelope<S>(stream: &mut S) -> protocol_rs::generated::Envelope
where
    S: tokio_stream::Stream<Item = Result<protocol_rs::generated::Envelope, tonic::Status>> + Unpin,
{
    use tokio_stream::StreamExt;
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("event arrives before timeout")
        .expect("stream stays healthy")
        .expect("stream yields an envelope")
}

#[tokio::test(flavor = "multi_thread")]
async fn phase1_contract_end_to_end() {
    let (addr, state) = spawn_host().await;
    let channel = Endpoint::from_shared(format!("http://{addr}"))
        .expect("valid endpoint")
        .connect()
        .await
        .expect("connect to host");
    let mut system = SystemServiceClient::new(channel.clone());
    let mut workspaces = WorkspaceServiceClient::new(channel.clone());
    let mut tasks = TaskServiceClient::new(channel);

    // Negotiation succeeds within the same major and intersects capabilities.
    let reply = system
        .negotiate(Request::new(hello(1, 9)))
        .await
        .expect("negotiation succeeds")
        .into_inner();
    assert_eq!(reply.negotiated_minor, 0);
    assert_eq!(reply.protocol, Some(ProtocolVersion { major: 1, minor: 0 }));
    assert_eq!(reply.capabilities.get("events"), Some(&true));

    // Differing majors are rejected with the structured error contract.
    let rejected = system
        .negotiate(Request::new(hello(2, 0)))
        .await
        .expect_err("major mismatch must fail");
    assert_eq!(rejected.code(), Code::FailedPrecondition);
    let detail = error_from_status(&rejected).expect("structured error in details");
    assert_eq!(detail.code(), ErrorCode::UnsupportedProtocolVersion);

    // Health reports SERVING.
    let health = system
        .health(Request::new(HealthRequest {}))
        .await
        .expect("health responds")
        .into_inner();
    assert_eq!(health.status(), ServingStatus::Serving);

    // Both list endpoints answer with empty stub pages.
    let page = workspaces
        .list(Request::new(ListWorkspacesRequest::default()))
        .await
        .expect("workspace list responds")
        .into_inner();
    assert!(page.workspaces.is_empty());
    assert!(page.pagination.is_none());
    let page = tasks
        .list(Request::new(ListTasksRequest::default()))
        .await
        .expect("task list responds")
        .into_inner();
    assert!(page.tasks.is_empty());
    assert!(page.pagination.is_none());

    // Event stream: pre-subscribe events are not delivered; live events are.
    let _before_subscribe = state.bus.publish("test.before", b"skipped");
    let mut events = system
        .subscribe_events(Request::new(SubscribeEventsRequest::default()))
        .await
        .expect("subscribe succeeds")
        .into_inner();
    let first = state.bus.publish("test.live", b"live-1");
    let received = next_envelope(&mut events).await;
    assert_eq!(received.sequence, first.sequence);

    // Cancellation is transport-level: dropping the RPC frees the stream and
    // the host keeps serving.
    drop(events);
    let health = system
        .health(Request::new(HealthRequest {}))
        .await
        .expect("host still healthy after client cancels stream")
        .into_inner();
    assert_eq!(health.status(), ServingStatus::Serving);

    // A valid reconnect token resumes past the last acknowledged sequence.
    let stream_id = state.bus.stream_id().to_owned();
    let mut resumed = system
        .subscribe_events(Request::new(reconnect_token(
            stream_id.clone(),
            first.sequence.expect("sequence assigned"),
            Duration::from_secs(60),
        )))
        .await
        .expect("resume succeeds")
        .into_inner();
    let second = state.bus.publish("test.live", b"live-2");
    let received = next_envelope(&mut resumed).await;
    assert_eq!(received.sequence, second.sequence);

    // Expired tokens cannot replay: ERROR_CODE_STREAM_GAP.
    let rejected = system
        .subscribe_events(Request::new(reconnect_token(
            stream_id.clone(),
            second.sequence.unwrap(),
            Duration::from_secs(0),
        )))
        .await
        .expect_err("expired token must fail");
    assert_eq!(rejected.code(), Code::FailedPrecondition);
    assert_eq!(
        error_from_status(&rejected)
            .expect("structured error")
            .code(),
        ErrorCode::StreamGap
    );

    // Sequences evicted from the bounded buffer are unreplayable: STREAM_GAP.
    for _ in 0..128 {
        state.bus.publish("test.fill", b"fill");
    }
    let rejected = system
        .subscribe_events(Request::new(reconnect_token(
            stream_id.clone(),
            0,
            Duration::from_secs(60),
        )))
        .await
        .expect_err("evicted replay window must fail");
    assert_eq!(
        error_from_status(&rejected)
            .expect("structured error")
            .code(),
        ErrorCode::StreamGap
    );

    // Client-controlled sequence arithmetic must never overflow the Host.
    let rejected = system
        .subscribe_events(Request::new(reconnect_token(
            stream_id.clone(),
            u64::MAX,
            Duration::from_secs(60),
        )))
        .await
        .expect_err("overflowing sequence must fail");
    assert_eq!(
        error_from_status(&rejected)
            .expect("structured error")
            .code(),
        ErrorCode::StreamGap
    );

    // Duplicate idempotency keys run the mutation once through the shared
    // store the running server itself holds.
    let executions = AtomicUsize::new(0);
    let produce = || {
        executions.fetch_add(1, Ordering::SeqCst);
        b"response".to_vec()
    };
    let (first_payload, first_fresh) = state.idempotency.execute("task-create-42", produce);
    let (second_payload, second_fresh) = state.idempotency.execute("task-create-42", produce);
    assert!(first_fresh);
    assert!(!second_fresh);
    assert_eq!(first_payload, second_payload);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}
