//! Handshake and capability negotiation (plan section 9.2).
//!
//! Negotiation rules:
//! - major versions must match exactly;
//! - `negotiated_minor = min(client.minor, host.minor)`;
//! - a capability is enabled only when both sides advertise it.

use crate::generated::ProtocolVersion;
pub use crate::generated::{Auth, AuthKind, ClientHello, HostReply};
use prost::Message;
use std::collections::HashMap;
use std::fmt;

/// Wire protocol major version implemented by this crate.
pub const PROTOCOL_MAJOR: i32 = 1;
/// Wire protocol minor version implemented by this crate.
pub const PROTOCOL_MINOR: i32 = 0;

/// The protocol version this crate speaks.
pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion {
    major: PROTOCOL_MAJOR,
    minor: PROTOCOL_MINOR,
};

/// Why a handshake cannot proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    /// Majors differ; no compatibility exists (plan section 9.4).
    UnsupportedMajor { client_major: i32, host_major: i32 },
}

impl fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NegotiationError::UnsupportedMajor {
                client_major,
                host_major,
            } => {
                write!(
                    f,
                    "unsupported protocol version: client speaks {client_major}.x, host speaks {host_major}.x"
                )
            }
        }
    }
}

impl std::error::Error for NegotiationError {}

/// Returns the negotiated minor version for a successful handshake.
///
/// An older minor client interoperates with a newer minor host (and vice
/// versa) because both fall back to the lower minor; differing majors are
/// rejected with [`NegotiationError::UnsupportedMajor`].
pub fn negotiate(
    client: &ProtocolVersion,
    host: &ProtocolVersion,
) -> Result<i32, NegotiationError> {
    if client.major != host.major {
        return Err(NegotiationError::UnsupportedMajor {
            client_major: client.major,
            host_major: host.major,
        });
    }
    Ok(client.minor.min(host.minor))
}

/// AND-combines capability maps: a capability is enabled only when both
/// sides advertise it as `true`. Capabilities only one side knows stay out
/// of the result.
pub fn intersect_capabilities(
    client: &HashMap<String, bool>,
    host: &HashMap<String, bool>,
) -> HashMap<String, bool> {
    client
        .iter()
        .filter_map(|(name, client_enabled)| {
            host.get(name)
                .map(|host_enabled| (name.clone(), *client_enabled && *host_enabled))
        })
        .collect()
}

/// Maps a failed negotiation onto the wire error contract: gRPC
/// `FAILED_PRECONDITION` whose details encode a `lazarus.protocol.v1.Error`
/// with `ERROR_CODE_UNSUPPORTED_PROTOCOL_VERSION`, so any client can detect
/// the failure precisely.
impl From<NegotiationError> for tonic::Status {
    fn from(err: NegotiationError) -> Self {
        let message = err.to_string();
        let detail = crate::generated::Error {
            code: crate::generated::ErrorCode::UnsupportedProtocolVersion as i32,
            message: message.clone(),
            details: Vec::new(),
        };
        tonic::Status::with_details(
            tonic::Code::FailedPrecondition,
            message,
            detail.encode_to_vec().into(),
        )
    }
}

/// Decodes the structured [`crate::error::Error`] carried in a status'
/// details bytes, if the peer attached one.
pub fn error_from_status(status: &tonic::Status) -> Option<crate::generated::Error> {
    crate::generated::Error::decode(status.details()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::ErrorCode;

    fn version(major: i32, minor: i32) -> ProtocolVersion {
        ProtocolVersion { major, minor }
    }

    #[test]
    fn older_minor_client_negotiates_with_newer_host() {
        assert_eq!(negotiate(&version(1, 4), &version(1, 6)).unwrap(), 4);
    }

    #[test]
    fn newer_minor_client_negotiates_with_older_host() {
        assert_eq!(negotiate(&version(1, 6), &version(1, 4)).unwrap(), 4);
    }

    #[test]
    fn unsupported_major_fails_clearly() {
        let err = negotiate(&version(2, 0), &version(1, 6)).unwrap_err();
        assert_eq!(
            err,
            NegotiationError::UnsupportedMajor {
                client_major: 2,
                host_major: 1
            }
        );

        let status = tonic::Status::from(err);
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        let decoded = error_from_status(&status).expect("structured error in details");
        assert_eq!(decoded.code(), ErrorCode::UnsupportedProtocolVersion);
    }

    #[test]
    fn capabilities_intersect_with_and_semantics() {
        let client = HashMap::from([
            ("pty".to_string(), true),
            ("containers".to_string(), true),
            ("remote_runner".to_string(), true),
        ]);
        let host = HashMap::from([
            ("pty".to_string(), false),
            ("containers".to_string(), true),
            ("gpu".to_string(), true),
        ]);
        let effective = intersect_capabilities(&client, &host);
        // AND semantics: shared-but-disabled stays present as false,
        // host-only capabilities stay out.
        assert_eq!(
            effective,
            HashMap::from([("containers".to_string(), true), ("pty".to_string(), false)])
        );
    }
}
