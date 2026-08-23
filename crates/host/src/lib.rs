//! Lazarus local Host daemon library: the Phase 1.5 Axum JSON/HTTP surface
//! (system info, health, SSE event subscription at `/system/events`,
//! workspaces, tasks) shared by the binary and integration tests.

mod events;
mod services;

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

pub use events::{EventBus, EventFrame, TaskSummary, WorkspaceSummary};
use protocol_rs::auth::LOCAL_TOKEN_ENV;
use protocol_rs::idempotency::MemoryIdempotencyStore;
pub use services::{
    GateCode, GateError, HostServices, LAST_OUTAGE_HEADER, authorize_and_negotiate, build_router,
    transport_gate,
};

/// Why the daemon refused to start serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupConfigError {
    /// The configured listen address is not loopback; Phase 1 serves local
    /// clients only.
    NonLoopbackListenAddr(SocketAddr),
    /// `{LOCAL_TOKEN_ENV}` is unset.
    MissingLocalToken,
    /// `{LOCAL_TOKEN_ENV}` is set but empty.
    EmptyLocalToken,
}

impl fmt::Display for StartupConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonLoopbackListenAddr(addr) => {
                write!(
                    f,
                    "listen address {addr} is not loopback; refusing to serve non-local clients"
                )
            }
            // Never echo the token value itself into diagnostics.
            Self::MissingLocalToken => write!(
                f,
                "{LOCAL_TOKEN_ENV} is not set; generate a per-install secret before starting the Host"
            ),
            Self::EmptyLocalToken => {
                write!(f, "{LOCAL_TOKEN_ENV} is set but empty")
            }
        }
    }
}

impl std::error::Error for StartupConfigError {}

/// Rejects listen addresses that would expose the daemon beyond loopback.
pub fn validate_loopback_addr(addr: SocketAddr) -> Result<(), StartupConfigError> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(StartupConfigError::NonLoopbackListenAddr(addr))
    }
}

/// Validates a raw local-token value from [`resolve_local_token`].
fn resolve_local_token(raw: Option<&str>) -> Result<Arc<str>, StartupConfigError> {
    match raw.map(str::trim) {
        None => Err(StartupConfigError::MissingLocalToken),
        Some("") => Err(StartupConfigError::EmptyLocalToken),
        Some(token) => Ok(Arc::from(token)),
    }
}

/// Loads the per-install local token from the environment. The Host refuses
/// to serve without a non-empty token.
pub fn local_token_from_env() -> Result<Arc<str>, StartupConfigError> {
    resolve_local_token(std::env::var(LOCAL_TOKEN_ENV).ok().as_deref())
}

/// State shared behind every Host RPC service.
pub struct HostState {
    /// Event bus backing the `/system/events` SSE subscription.
    pub bus: EventBus,
    /// Process-local idempotency store shared by all write paths.
    pub idempotency: MemoryIdempotencyStore,
    host_capabilities: HashMap<String, bool>,
}

impl HostState {
    pub fn new() -> Self {
        Self::with_event_capacity(1024)
    }

    pub fn with_event_capacity(event_capacity: usize) -> Self {
        Self {
            bus: EventBus::with_event_capacity(event_capacity),
            idempotency: MemoryIdempotencyStore::new(),
            host_capabilities: HashMap::from([("events".to_owned(), true)]),
        }
    }

    pub fn host_capabilities(&self) -> &HashMap<String, bool> {
        &self.host_capabilities
    }
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_listen_addr() {
        for addr in ["0.0.0.0:50051", "192.168.1.10:50051", "[::]:50051"] {
            let addr: SocketAddr = addr.parse().expect("parseable socket address");
            assert_eq!(
                validate_loopback_addr(addr),
                Err(StartupConfigError::NonLoopbackListenAddr(addr))
            );
        }
        let loopback: SocketAddr = "127.0.0.1:50051".parse().unwrap();
        assert_eq!(validate_loopback_addr(loopback), Ok(()));
        let v6: SocketAddr = "[::1]:50051".parse().unwrap();
        assert_eq!(validate_loopback_addr(v6), Ok(()));
    }

    #[test]
    fn rejects_missing_or_empty_local_token() {
        assert_eq!(
            resolve_local_token(None),
            Err(StartupConfigError::MissingLocalToken)
        );
        assert_eq!(
            resolve_local_token(Some("")),
            Err(StartupConfigError::EmptyLocalToken)
        );
        assert_eq!(
            resolve_local_token(Some("   ")),
            Err(StartupConfigError::EmptyLocalToken)
        );
        let token = resolve_local_token(Some(" s3cret ")).expect("valid token");
        assert_eq!(&*token, "s3cret");
        // The error display must never contain the token value.
        assert!(
            !StartupConfigError::EmptyLocalToken
                .to_string()
                .contains("s3cret")
        );
    }
}
