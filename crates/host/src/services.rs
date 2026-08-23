//! tonic service implementations for the Phase 1 Host surface.

use std::sync::Arc;

use prost::Message as _;
use protocol_rs::envelope::Envelope;
use protocol_rs::error::{Error, ErrorCode};
use protocol_rs::generated::{
    ClientHello, GetInfoRequest, GetInfoResponse, HealthRequest, HealthResponse, HostReply,
    ListTasksRequest, ListTasksResponse, ListWorkspacesRequest, ListWorkspacesResponse,
    ServingStatus, SubscribeEventsRequest, system_service_server::SystemService,
    task_service_server::TaskService, workspace_service_server::WorkspaceService,
};
use protocol_rs::handshake::{CURRENT_PROTOCOL_VERSION, intersect_capabilities, negotiate};
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status};

use crate::HostState;

/// Implements System, Workspace, and Task services over shared [`HostState`].
#[derive(Clone)]
pub struct HostServices {
    state: Arc<HostState>,
}

impl HostServices {
    pub fn new(state: Arc<HostState>) -> Self {
        Self { state }
    }
}

fn stream_gap(reason: String) -> Status {
    let detail = Error {
        code: ErrorCode::StreamGap as i32,
        message: reason.clone(),
        details: Vec::new(),
    };
    Status::with_details(
        Code::FailedPrecondition,
        reason,
        detail.encode_to_vec().into(),
    )
}

#[tonic::async_trait]
impl SystemService for HostServices {
    async fn negotiate(
        &self,
        request: Request<ClientHello>,
    ) -> Result<Response<HostReply>, Status> {
        let hello = request.into_inner();
        let client_protocol = hello.protocol.unwrap_or_default();
        let negotiated_minor = negotiate(&client_protocol, &CURRENT_PROTOCOL_VERSION)?;
        let client_capabilities = hello
            .supported_features
            .iter()
            .map(|feature| (feature.clone(), true))
            .collect();
        Ok(Response::new(HostReply {
            host_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: Some(CURRENT_PROTOCOL_VERSION),
            negotiated_minor,
            capabilities: intersect_capabilities(
                &client_capabilities,
                self.state.host_capabilities(),
            ),
        }))
    }

    async fn get_info(
        &self,
        _request: Request<GetInfoRequest>,
    ) -> Result<Response<GetInfoResponse>, Status> {
        Ok(Response::new(GetInfoResponse {
            host_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: Some(CURRENT_PROTOCOL_VERSION),
            capabilities: self.state.host_capabilities().clone(),
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

    type SubscribeEventsStream = ReceiverStream<Result<Envelope, Status>>;

    async fn subscribe_events(
        &self,
        request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let request = request.into_inner();
        let bus = &self.state.bus;
        // Subscribe before snapshotting replay so nothing published after the
        // snapshot is lost; the forwarder then deduplicates by sequence.
        let live = bus.subscribe();
        let replay = request
            .reconnect
            .as_ref()
            .map(|token| bus.resume(token).map_err(stream_gap))
            .transpose()?
            .map(|resume| resume.envelopes)
            .unwrap_or_default();
        let (tx, rx) = tokio::sync::mpsc::channel(1024);
        tokio::spawn(async move {
            let mut live = live;
            let mut last_sent = 0;
            for envelope in replay {
                last_sent = last_sent.max(envelope.sequence.unwrap_or_default());
                if tx.send(Ok(envelope)).await.is_err() {
                    return;
                }
            }
            loop {
                match live.recv().await {
                    Ok(envelope) => {
                        let sequence = envelope.sequence.unwrap_or_default();
                        if sequence <= last_sent {
                            continue;
                        }
                        last_sent = sequence;
                        if tx.send(Ok(envelope)).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        let _ = tx
                            .send(Err(stream_gap(
                                "host event buffer advanced past this subscriber".to_owned(),
                            )))
                            .await;
                        break;
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[tonic::async_trait]
impl WorkspaceService for HostServices {
    async fn list(
        &self,
        _request: Request<ListWorkspacesRequest>,
    ) -> Result<Response<ListWorkspacesResponse>, Status> {
        Ok(Response::new(ListWorkspacesResponse::default()))
    }
}

#[tonic::async_trait]
impl TaskService for HostServices {
    async fn list(
        &self,
        _request: Request<ListTasksRequest>,
    ) -> Result<Response<ListTasksResponse>, Status> {
        Ok(Response::new(ListTasksResponse::default()))
    }
}
