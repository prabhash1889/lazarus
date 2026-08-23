pub use crate::generated::{Envelope, ProtocolVersion, ReconnectToken};

impl ReconnectToken {
    /// True when the token cannot be used to resume: it is expired, or it
    /// carries no expiry at all (a token without an expiry is malformed and
    /// treated as expired).
    pub fn usable_at(&self, now: std::time::SystemTime) -> bool {
        match &self.expires_at {
            Some(expires_at) => matches!(
                std::time::SystemTime::try_from(*expires_at),
                Ok(expires) if expires > now
            ),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn token(expires_in: Option<Duration>) -> ReconnectToken {
        ReconnectToken {
            token: "t".into(),
            stream_id: "s".into(),
            last_sequence: 7,
            expires_at: expires_in.map(|d| prost_types::Timestamp::from(SystemTime::now() + d)),
        }
    }

    #[test]
    fn unexpired_token_is_usable() {
        assert!(token(Some(Duration::from_secs(60))).usable_at(SystemTime::now()));
    }

    #[test]
    fn expired_or_malformed_token_is_unusable() {
        let base = SystemTime::now();
        let expired = ReconnectToken {
            token: "t".into(),
            stream_id: "s".into(),
            last_sequence: 7,
            expires_at: Some(prost_types::Timestamp::from(base - Duration::from_secs(1))),
        };
        assert!(!expired.usable_at(base));
        assert!(!token(None).usable_at(base));
    }
}
