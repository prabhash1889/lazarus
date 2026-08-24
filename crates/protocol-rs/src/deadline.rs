//! Transport-neutral cancellation and deadline contract.
//!
//! A caller expresses how long its request may keep working by attaching an
//! absolute deadline - Unix epoch milliseconds - in the [`DEADLINE_HEADER`]
//! header on any unary or streaming call. Absolute timestamps are
//! transport-neutral: every hop reads the same value instead of translating
//! per-hop budgets.
//!
//! Operational rules:
//! - a Host must stop working at the deadline and answer (or close the
//!   stream with) the canonical `DEADLINE_EXCEEDED` error;
//! - closing the connection is immediate cancellation for whatever the
//!   deadline does not cover;
//! - a malformed deadline header is an `INVALID_ARGUMENT`, never silently
//!   ignored, so callers cannot believe a budget exists when it does not;
//! - clients derive the header from [`DEFAULT_RPC_BUDGET_MS`] and apply the
//!   same local budget to their own transport timeout plus a small grace so
//!   the Host's typed `DEADLINE_EXCEEDED` wins the race.

/// Header carrying the absolute deadline as epoch milliseconds.
pub const DEADLINE_HEADER: &str = "x-lazarus-deadline";

/// The shared per-call work budget CLI and Desktop both use: they stamp
/// deadlines `DEFAULT_RPC_BUDGET_MS` ahead and time their own transports
/// out at the same point (plus [`CLIENT_TIMEOUT_GRACE_MS`], so the Host's
/// typed rejection arrives before the client aborts).
pub const DEFAULT_RPC_BUDGET_MS: u64 = 5_000;

/// Extra time the client's local timeout waits beyond the stamped deadline.
pub const CLIENT_TIMEOUT_GRACE_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineError {
    /// The header value was not a non-negative integer.
    Malformed,
    /// The deadline already elapsed at parse time (`now_ms`).
    Expired { now_ms: u64 },
}

impl std::fmt::Display for DeadlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(
                f,
                "{DEADLINE_HEADER} must be a Unix epoch timestamp in milliseconds"
            ),
            Self::Expired { now_ms } => {
                write!(f, "{DEADLINE_HEADER} elapsed before {now_ms}")
            }
        }
    }
}

impl std::error::Error for DeadlineError {}

/// An absolute caller deadline in Unix epoch milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    epoch_ms: u64,
}

impl Deadline {
    /// Parses a raw header value against the current time. An expired
    /// deadline is rejected here rather than at first use so cancellation is
    /// observable even when no further work remains.
    pub fn parse(raw: &str, now_ms: u64) -> Result<Self, DeadlineError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DeadlineError::Malformed);
        }
        let epoch_ms: u64 = trimmed.parse().map_err(|_| DeadlineError::Malformed)?;
        if epoch_ms <= now_ms {
            return Err(DeadlineError::Expired { now_ms });
        }
        Ok(Self { epoch_ms })
    }

    /// The remaining budget in whole milliseconds; always positive at parse
    /// time, shrinking towards zero as the wall clock advances.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.epoch_ms.saturating_sub(now_ms)
    }

    /// True once the deadline has passed: the Host must have stopped work.
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.epoch_ms
    }

    /// Formats a deadline for [`DEADLINE_HEADER`] from a budget starting
    /// now, the way every client stamps outgoing requests.
    pub fn header_from_budget(now_ms: u64, budget_ms: u64) -> String {
        (now_ms.saturating_add(budget_ms)).to_string()
    }
}

/// Current Unix epoch milliseconds; injected as `now_ms` everywhere else so
/// the contract stays testable without sleeping.
pub fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    #[test]
    fn parses_future_deadlines_and_rejects_the_rest() {
        let deadline = Deadline::parse(&format!("{}", NOW + 500), NOW).expect("future");
        assert_eq!(deadline.remaining_ms(NOW), 500);
        assert!(!deadline.is_expired(NOW));

        assert_eq!(
            Deadline::parse("not-a-number", NOW),
            Err(DeadlineError::Malformed)
        );
        assert_eq!(Deadline::parse("", NOW), Err(DeadlineError::Malformed));
        assert_eq!(Deadline::parse("-50", NOW), Err(DeadlineError::Malformed));
        assert_eq!(Deadline::parse("1.5", NOW), Err(DeadlineError::Malformed));
        // Elapsed budgets are refused immediately, not deferred to use.
        assert_eq!(
            Deadline::parse(&format!("{}", NOW), NOW),
            Err(DeadlineError::Expired { now_ms: NOW })
        );
        assert!(Deadline::parse(&NOW.to_string(), NOW + 1).is_err());
    }

    #[test]
    fn expiry_tracks_the_wall_clock_and_remaining_never_underflows() {
        let deadline = Deadline::parse(&(NOW + 100).to_string(), NOW).expect("future");
        assert_eq!(deadline.remaining_ms(NOW + 40), 60);
        assert_eq!(deadline.remaining_ms(NOW + 400), 0);
        assert!(!deadline.is_expired(NOW + 99));
        assert!(deadline.is_expired(NOW + 100));
    }

    #[test]
    fn header_roundtrips_through_parse_for_any_budget() {
        for budget in [1, DEFAULT_RPC_BUDGET_MS, 120_000] {
            let raw = Deadline::header_from_budget(NOW, budget);
            let parsed = Deadline::parse(&raw, NOW).expect("stamped header parses");
            assert_eq!(parsed.epoch_ms, NOW + budget);
            assert_eq!(parsed.remaining_ms(NOW), budget);
        }
        assert_eq!(
            Deadline::header_from_budget(NOW, DEFAULT_RPC_BUDGET_MS),
            format!("{}", NOW + DEFAULT_RPC_BUDGET_MS)
        );
    }

    #[test]
    fn unix_now_is_plausible() {
        assert!(unix_now_ms() > 1_700_000_000_000);
    }
}
