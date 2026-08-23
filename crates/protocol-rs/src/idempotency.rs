//! Idempotency-key contract for write RPCs (plan section 9.1).
//!
//! Write RPCs carry a client-chosen idempotency key in gRPC metadata under
//! [`IDEMPOTENCY_KEY_HEADER`]. Hosts must treat a repeated key as a replay
//! of the original mutation: return the original response instead of
//! executing the mutation again. Keys are deliberately metadata, not message
//! fields, so every write RPC inherits the convention additively.

use std::collections::HashMap;
use std::sync::Mutex;

/// gRPC metadata header carrying client-chosen idempotency keys.
pub const IDEMPOTENCY_KEY_HEADER: &str = "x-lazarus-idempotency-key";

/// Attaches an idempotency key to an outgoing request.
pub fn attach_idempotency_key<T>(request: &mut tonic::Request<T>, key: impl Into<String>) {
    let key = key.into();
    assert!(!key.is_empty(), "idempotency key must not be empty");
    let value = key.parse().expect("idempotency key is valid header value");
    request.metadata_mut().insert(IDEMPOTENCY_KEY_HEADER, value);
}

/// Extracts the idempotency key from an incoming request, if any.
pub fn extract_idempotency_key<T>(request: &tonic::Request<T>) -> Option<String> {
    request
        .metadata()
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// Process-local store that executes a mutation at most once per idempotency
/// key within the host's lifetime. The durable, cross-restart store lives in
/// the Host's SQLite persistence; this primitive exists so protocol-level
/// semantics can be tested and reused by any in-process service.
#[derive(Default)]
pub struct MemoryIdempotencyStore {
    completed: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryIdempotencyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `produce` at most once per key. Returns the payload together
    /// with whether it was freshly produced: when `fresh` is `false` the key
    /// was seen before, `produce` was NOT executed, and callers must return
    /// the cached payload instead of mutating state again.
    pub fn execute<F>(&self, key: &str, produce: F) -> (Vec<u8>, bool)
    where
        F: FnOnce() -> Vec<u8>,
    {
        let mut completed = self.completed.lock().expect("idempotency store lock");
        if let Some(payload) = completed.get(key) {
            return (payload.clone(), false);
        }
        let payload = produce();
        completed.insert(key.to_owned(), payload.clone());
        (payload, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn duplicate_idempotency_key_does_not_duplicate_mutation() {
        let store = MemoryIdempotencyStore::new();
        let mutations = AtomicUsize::new(0);

        let (first, fresh) = store.execute("task-create-1", || {
            mutations.fetch_add(1, Ordering::SeqCst);
            b"response-v1".to_vec()
        });
        assert!(fresh);

        // Same key replays the recorded response without re-running the
        // mutation, even if the producer would now yield different bytes.
        let (second, fresh) = store.execute("task-create-1", || {
            mutations.fetch_add(1, Ordering::SeqCst);
            b"response-v2".to_vec()
        });
        assert!(!fresh);
        assert_eq!(second, first);
        assert_eq!(mutations.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn distinct_keys_execute_independently() {
        let store = MemoryIdempotencyStore::new();
        let (first, first_fresh) = store.execute("a", || vec![1]);
        let (second, second_fresh) = store.execute("b", || vec![2]);
        assert!(first_fresh && second_fresh);
        assert_ne!(first, second);
    }

    #[test]
    fn key_travels_through_tonic_metadata() {
        let mut request = tonic::Request::new(());
        assert!(extract_idempotency_key(&request).is_none());

        attach_idempotency_key(&mut request, "create-task-42");
        assert_eq!(
            extract_idempotency_key(&request).as_deref(),
            Some("create-task-42")
        );
    }

    #[test]
    #[should_panic(expected = "idempotency key must not be empty")]
    fn empty_key_is_rejected() {
        let mut request = tonic::Request::new(());
        attach_idempotency_key(&mut request, "");
    }
}
