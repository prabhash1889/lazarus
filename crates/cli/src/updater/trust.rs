//! The pinned release trust root.
//!
//! The updater only accepts manifests signed by the release key whose
//! public half is compiled into this binary. There is deliberately no
//! override flag: trusting a different key is a new build, not a
//! runtime decision, so a tampered environment can never widen what an
//! install accepts.

use ed25519_dalek::VerifyingKey;

/// Ed25519 public key of the Lazarus release signing key (hex).
const RELEASE_TRUST_ROOT_HEX: &str =
    "38b4d8fae5dac10eb0cadd120e386afdbc46c5db1a3fc9a330350436fb033549";

/// Decodes the pinned release key. Fails loudly at call time rather than
/// panicking during const evaluation if the constant were ever corrupted.
pub fn release_trust_root() -> anyhow::Result<VerifyingKey> {
    decode_trust_root(RELEASE_TRUST_ROOT_HEX)
        .map_err(|error| anyhow::anyhow!("the pinned release trust root is invalid: {error}"))
}

pub fn decode_trust_root(hex: &str) -> Result<VerifyingKey, String> {
    if hex.len() != 64 {
        return Err(format!("expected 64 hex characters, got {}", hex.len()));
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = (chunk[0] as char).to_digit(16).ok_or("non-hex digit")? as u8;
        let low = (chunk[1] as char).to_digit(16).ok_or("non-hex digit")? as u8;
        bytes[index] = (high << 4) | low;
    }
    VerifyingKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_key_decodes_and_round_trips() {
        let key = release_trust_root().expect("pinned key decodes");
        assert_eq!(
            crate::updater::download::hex_encode(&key.to_bytes()),
            RELEASE_TRUST_ROOT_HEX
        );
    }

    #[test]
    fn malformed_pinned_keys_are_refused() {
        assert!(decode_trust_root("abcd").is_err());
        assert!(decode_trust_root(&"zz".repeat(32)).is_err());
    }
}
