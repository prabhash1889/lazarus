//! Idempotency-key contract for write methods (plan section 9.1).
//!
//! Write methods carry a client-chosen idempotency key in the request
//! header [`IDEMPOTENCY_KEY_HEADER`]. Hosts must treat a repeated key as a
//! replay of the original mutation: return the original response instead of
//! executing the mutation again. Keys are deliberately headers, not message
//! fields, so every write method inherits the convention additively.

use std::collections::HashMap;
use std::sync::Mutex;

/// Request header carrying client-chosen idempotency keys.
pub const IDEMPOTENCY_KEY_HEADER: &str = "x-lazarus-idempotency-key";

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
}
