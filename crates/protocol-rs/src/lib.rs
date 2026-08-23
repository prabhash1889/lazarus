//! Rust bindings for the Lazarus Protocol (wire version 1.x).
//!
//! Types under [`generated`] are produced from `proto/*.proto` by the build
//! script. Do not edit generated output by hand; run `cargo build` (Rust) or
//! `pnpm gen:protocol` (TypeScript) to refresh it.
use std::time::SystemTime;

#[allow(clippy::all)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/lazarus.protocol.v1.rs"));
}

/// Envelope framing and stream-resume types (plan section 9.3).
pub mod envelope;
/// Error model shared by all RPCs.
pub mod error;
/// Handshake and capability negotiation types (plan section 9.2).
pub mod handshake;
/// Idempotency-key contract for write RPCs (plan section 9.1).
pub mod idempotency;
/// Cursor pagination request/response types.
pub mod pagination;

pub use generated::{
    system_service_client::SystemServiceClient, system_service_server::SystemServiceServer,
    task_service_client::TaskServiceClient, task_service_server::TaskServiceServer,
    workspace_service_client::WorkspaceServiceClient,
    workspace_service_server::WorkspaceServiceServer, GetInfoRequest, GetInfoResponse,
    HealthRequest, HealthResponse, ListTasksRequest, ListTasksResponse, ListWorkspacesRequest,
    ListWorkspacesResponse, SubscribeEventsRequest, TaskStatus,
};

/// Builds a minimal valid envelope stamped with the current time.
pub fn new_envelope(message_id: impl Into<String>) -> generated::Envelope {
    generated::Envelope {
        message_id: message_id.into(),
        timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
        ..generated::Envelope::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn envelope_roundtrips() {
        let original = new_envelope("m-1");
        let decoded =
            generated::Envelope::decode(original.encode_to_vec().as_slice()).expect("decode");
        assert_eq!(decoded.message_id, "m-1");
    }

    #[test]
    fn protocol_version_roundtrips() {
        let version = generated::ProtocolVersion { major: 1, minor: 0 };
        assert_eq!(
            generated::ProtocolVersion::decode(version.encode_to_vec().as_slice()).unwrap(),
            version
        );
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        // Field 99 is not part of ReconnectToken; decoding must ignore it.
        let mut buf = Vec::new();
        prost::encoding::encode_key(99, prost::encoding::WireType::Varint, &mut buf);
        prost::encoding::encode_varint(1, &mut buf);
        assert!(generated::ReconnectToken::decode(buf.as_slice()).is_ok());
    }
}
