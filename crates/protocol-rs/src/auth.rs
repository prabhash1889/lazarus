//! Local authentication primitives shared by Host, CLI, and Desktop (Phase 1).
//!
//! Credential contract:
//!
//! - The per-install local token lives in the `LAZARUS_LOCAL_TOKEN`
//!   environment variable, provisioned by whatever installs or launches the
//!   Host daemon. This is the smallest practical source both the CLI and the
//!   Desktop app inherit automatically when they spawn the Host as a child
//!   process; it requires no files, keychain integration, or extra flags.
//! - Every request must carry the token as metadata
//!   `authorization: Bearer <token>`.
//! - The Host refuses to serve when the variable is unset or empty, and
//!   rejects any request whose metadata does not match with UNAUTHENTICATED.
//! - Comparison is constant-time; the token value must never appear in logs
//!   or error messages.

/// Environment variable holding the per-install local token (Phase 1 source).
pub const LOCAL_TOKEN_ENV: &str = "LAZARUS_LOCAL_TOKEN";

/// Request metadata key carrying the local token.
pub const AUTH_METADATA_KEY: &str = "authorization";

/// Prefix required on the [`AUTH_METADATA_KEY`] value.
const BEARER_PREFIX: &str = "Bearer ";

/// Equality comparison over bytes that runs in time independent of where the
/// first difference occurs, so timing cannot leak how much of a presented
/// token matched. Length differences still short-circuit; only lengths are
/// compared, never secret contents.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Builds the exact `authorization` metadata value clients must send.
pub fn bearer_header(token: &str) -> String {
    format!("{BEARER_PREFIX}{token}")
}

/// Validates a presented `authorization` metadata value against the
/// configured local token. Returns true only for a well-formed
/// `Bearer <token>` value whose token matches byte-for-byte.
pub fn verify_bearer_header(expected_token: &str, provided: Option<&str>) -> bool {
    let Some(provided) = provided.and_then(|value| value.strip_prefix(BEARER_PREFIX)) else {
        return false;
    };
    constant_time_eq(expected_token.as_bytes(), provided.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_only_equal_slices() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn verifies_bearer_headers() {
        assert!(verify_bearer_header("tok", Some("Bearer tok")));
        assert!(!verify_bearer_header("tok", Some("Bearer toK")));
        assert!(!verify_bearer_header("tok", Some("bearer tok")));
        assert!(!verify_bearer_header("tok", Some("tok")));
        assert!(!verify_bearer_header("tok", None));
    }
}
