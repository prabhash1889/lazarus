//! Server-sent event delivery backing `GET /system/events` (Phase 1.5
//! scope: an outage tombstone plus an authoritative snapshot on every
//! subscription, then live sequenced frames; nothing is retained or
//! replayed, so lagging subscribers are disconnected and resubscribe).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

const BROADCAST_CAPACITY: usize = 1024;

static NEXT_OUTAGE_ID: AtomicU64 = AtomicU64::new(1);

fn generate_outage_id() -> String {
    let id = NEXT_OUTAGE_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("outage-{nanos:x}-{id}")
}

/// A minimal workspace summary carried in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
}

/// A minimal task summary carried in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
}

/// The JSON frame shapes delivered over the SSE stream.
///
/// Every subscription starts with [`EventFrame::Outage`] (unless the client
/// proves it already knows about this exact outage) and an authoritative
/// [`EventFrame::Snapshot`], after which only [`EventFrame::Live`] frames
/// flow. There is no replay: a client that falls behind is disconnected and
/// must resubscribe for a fresh snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EventFrame {
    /// Marks the start of the current Host incarnation; clients compare it
    /// against the last outage they observed to detect a Host restart.
    Outage { outage_id: String },
    /// The authoritative current state; clients replace everything they
    /// know with it.
    Snapshot {
        workspaces: Vec<WorkspaceSummary>,
        tasks: Vec<TaskSummary>,
    },
    /// A live sequenced change since the snapshot.
    Live { sequence: u64 },
}

impl EventFrame {
    /// The restart tombstone for this bus's incarnation.
    pub(crate) fn tombstone(outage_id: &str) -> Self {
        Self::Outage {
            outage_id: outage_id.to_owned(),
        }
    }

    /// The authoritative snapshot; Phase 1 state is empty.
    pub(crate) fn authoritative_snapshot() -> Self {
        Self::Snapshot {
            workspaces: Vec::new(),
            tasks: Vec::new(),
        }
    }

    pub(crate) fn encode(&self) -> String {
        serde_json::to_string(self).expect("event frame serializes")
    }
}

/// Publishes live sequenced [`EventFrame`]s to subscribers and identifies
/// the current Host incarnation with a stable outage id.
pub struct EventBus {
    outage_id: String,
    /// Next live sequence number to assign.
    next_sequence: AtomicU64,
    tx: broadcast::Sender<EventFrame>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_broadcast_capacity(BROADCAST_CAPACITY)
    }

    pub fn with_event_capacity(capacity: usize) -> Self {
        Self::with_broadcast_capacity(capacity.max(1))
    }

    fn with_broadcast_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            outage_id: generate_outage_id(),
            next_sequence: AtomicU64::new(1),
            tx,
        }
    }

    /// Identifier of the current Host incarnation; stable until the Host
    /// process restarts, which mints a fresh one.
    pub fn outage_id(&self) -> &str {
        &self.outage_id
    }

    /// Subscribes to live frames published after this call. The Phase 1
    /// snapshot is empty and live frames carry no state payload, so every
    /// queued frame must be delivered exactly once; nothing is skipped and
    /// nothing is replayed.
    pub fn subscribe(&self) -> broadcast::Receiver<EventFrame> {
        self.tx.subscribe()
    }

    /// Publishes one live frame with the next sequence number. Phase 1
    /// carries no payload, so there is no state change to apply.
    pub fn publish(&self) -> EventFrame {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let frame = EventFrame::Live { sequence };
        let _ = self.tx.send(frame.clone());
        frame
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Decides whether a reconnecting client still needs the current tombstone:
/// it is skipped only when the client names this exact outage as its last
/// observed one. Any other value (missing, stale, unknown) replays it.
pub(crate) fn needs_tombstone(last_outage_id: Option<&str>, current: &str) -> bool {
    last_outage_id != Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_assigns_increasing_sequences() {
        let bus = EventBus::new();
        let first = bus.publish();
        let second = bus.publish();
        assert_eq!(first, EventFrame::Live { sequence: 1 });
        assert_eq!(second, EventFrame::Live { sequence: 2 });
    }

    #[test]
    fn subscribers_receive_live_frames() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish();
        assert_eq!(
            rx.try_recv().expect("live frame"),
            EventFrame::Live { sequence: 1 }
        );
    }

    #[test]
    fn tombstone_dedupe_only_for_exact_current_outage() {
        assert!(!needs_tombstone(Some("outage-a"), "outage-a"));
        assert!(needs_tombstone(None, "outage-a"));
        assert!(needs_tombstone(Some("outage-old"), "outage-a"));
    }

    #[test]
    fn frames_serialize_with_camel_case_shapes() {
        let tombstone = EventFrame::tombstone("outage-1");
        assert_eq!(
            tombstone.encode(),
            r#"{"type":"outage","outageId":"outage-1"}"#
        );
        let snapshot = EventFrame::authoritative_snapshot();
        assert_eq!(
            snapshot.encode(),
            r#"{"type":"snapshot","workspaces":[],"tasks":[]}"#
        );
        assert_eq!(
            EventFrame::Live { sequence: 7 }.encode(),
            r#"{"type":"live","sequence":7}"#
        );

        for frame in [
            EventFrame::tombstone("outage-1"),
            EventFrame::authoritative_snapshot(),
            EventFrame::Live { sequence: 7 },
        ] {
            let value = serde_json::to_value(frame).expect("event frame serializes");
            protocol_rs::generated_registry::wire::decode_system_subscribe_events_response(&value)
                .expect("hand-written event frame matches the generated wire contract");
        }
    }

    #[test]
    fn each_bus_mints_a_distinct_outage_id() {
        let first = EventBus::new();
        let second = EventBus::new();
        assert_ne!(first.outage_id(), second.outage_id());
        assert_eq!(first.outage_id(), first.outage_id());
    }

    #[test]
    fn frames_published_before_the_subscription_are_not_replayed() {
        let bus = EventBus::new();
        bus.publish();
        let mut rx = bus.subscribe();
        assert!(
            rx.try_recv().is_err(),
            "no replay of pre-subscription frames"
        );
    }

    #[test]
    fn a_lagged_receiver_observes_lag_instead_of_unbounded_buffering() {
        let bus = EventBus::with_event_capacity(2);
        let mut rx = bus.subscribe();
        for _ in 0..4 {
            bus.publish();
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }
}
