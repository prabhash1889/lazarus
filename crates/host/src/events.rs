//! Bounded in-memory event stream backing `SubscribeEvents` (Phase 1 scope:
//! live delivery plus replay of the retained window; nothing durable).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use protocol_rs::envelope::{Envelope, ReconnectToken};
use tokio::sync::broadcast;

const BROADCAST_CAPACITY: usize = 1024;

static NEXT_BUS_ID: AtomicU64 = AtomicU64::new(1);

fn generate_stream_id() -> String {
    let id = NEXT_BUS_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("bus-{nanos:x}-{id}")
}

/// Publishes sequenced [`Envelope`]s to live subscribers and retains the most
/// recent `capacity` frames so reconnect tokens can resume within a window.
pub struct EventBus {
    stream_id: String,
    capacity: usize,
    next_sequence: AtomicU64,
    tx: broadcast::Sender<Envelope>,
    history: Mutex<VecDeque<Envelope>>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            stream_id: generate_stream_id(),
            capacity: capacity.max(1),
            next_sequence: AtomicU64::new(1),
            tx,
            history: Mutex::new(VecDeque::new()),
        }
    }

    /// Identifier stamped onto every envelope this bus publishes; reconnect
    /// tokens must reference it to be resumable.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Publishes one envelope, assigning its sequence and stream id.
    pub fn publish(&self, payload_type: &str, payload: &[u8]) -> Envelope {
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst);
        let envelope = Envelope {
            message_id: format!("{}/{}", self.stream_id, sequence),
            stream_id: Some(self.stream_id.clone()),
            sequence: Some(sequence),
            timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
            payload_type: payload_type.to_owned(),
            payload: payload.to_vec(),
            ..Envelope::default()
        };
        {
            let mut history = self.history.lock().expect("event history lock");
            history.push_back(envelope.clone());
            while history.len() > self.capacity {
                history.pop_front();
            }
        }
        let _ = self.tx.send(envelope.clone());
        envelope
    }

    /// Subscribes to live envelopes published after this call.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }

    /// Resolves which retained envelopes follow `token.last_sequence`.
    ///
    /// Returns an error reason when replay is impossible (expired or
    /// malformed token, unknown stream, or sequences evicted from the buffer);
    /// callers surface that as ERROR_CODE_STREAM_GAP.
    pub fn resume(&self, token: &ReconnectToken) -> Result<Resume, String> {
        if !token.usable_at(SystemTime::now()) {
            return Err("reconnect token is expired or has no expiry".to_owned());
        }
        if token.stream_id != self.stream_id {
            return Err("reconnect token references an unknown event stream".to_owned());
        }
        let next_sequence = self.next_sequence.load(Ordering::SeqCst);
        let oldest_retained = {
            let history = self.history.lock().expect("event history lock");
            history
                .front()
                .and_then(|envelope| envelope.sequence)
                .unwrap_or(next_sequence)
        };
        let requested_sequence = token
            .last_sequence
            .checked_add(1)
            .ok_or_else(|| "reconnect token sequence overflows".to_owned())?;
        if requested_sequence < oldest_retained {
            return Err(format!(
                "events before sequence {oldest_retained} are no longer retained"
            ));
        }
        if token.last_sequence >= next_sequence {
            return Err(format!(
                "reconnect token references unpublished sequence {}",
                token.last_sequence
            ));
        }
        let history = self.history.lock().expect("event history lock");
        Ok(Resume {
            envelopes: history
                .iter()
                .filter(|envelope| {
                    envelope
                        .sequence
                        .is_some_and(|seq| seq > token.last_sequence)
                })
                .cloned()
                .collect(),
        })
    }
}

/// Retained envelopes that should be replayed before live delivery resumes.
pub struct Resume {
    pub envelopes: Vec<Envelope>,
}
