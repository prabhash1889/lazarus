//! Lazarus local Host daemon library: the Phase 1 tonic service surface
//! (System, Workspace, Task) shared by the binary and integration tests.

mod events;
mod services;

use std::collections::HashMap;

pub use events::{EventBus, Resume};
use protocol_rs::idempotency::MemoryIdempotencyStore;
pub use services::HostServices;

/// State shared behind every Host RPC service.
pub struct HostState {
    /// Bounded in-memory event bus backing `SubscribeEvents`.
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
            bus: EventBus::new(event_capacity),
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
