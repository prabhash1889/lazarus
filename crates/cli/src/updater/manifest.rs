//! Signed release manifests.
//!
//! A manifest is a JSON object with the release description plus a detached
//! Ed25519 signature over the canonical serialization of everything else:
//!
//! ```json
//! {
//!   "schemaVersion": 1,
//!   "release": {
//!     "version": "0.2.0",
//!     "artifact": {
//!       "fileName": "lazarus-hostd",
//!       "sizeBytes": 1234,
//!       "sha256": "<64 hex chars>"
//!     }
//!   },
//!   "signature": { "algorithm": "ed25519", "value": "<base64>" }
//! }
//! ```
//!
//! Verification removes `signature`, serializes the remaining value with
//! sorted keys (serde_json's default map), and checks the Ed25519 signature
//! against the pinned release trust root. Any reordering or tampering
//! invalidates the signature, and unknown algorithms are refused before any
//! cryptographic work.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
const SIGNATURE_ALGORITHM: &str = "ed25519";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub release: Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub artifact: Artifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub file_name: String,
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the exact artifact bytes.
    pub sha256: String,
}

#[derive(Debug)]
pub enum ManifestError {
    Malformed(String),
    BadSignature(&'static str),
    InvalidContent(&'static str),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "release manifest is malformed: {detail}"),
            Self::BadSignature(reason) => {
                write!(
                    f,
                    "release manifest signature verification failed: {reason}"
                )
            }
            Self::InvalidContent(reason) => {
                write!(f, "signed release manifest is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// Verifies `bytes` against `trust_root` and returns the typed manifest on
/// success. The error type distinguishes tampering from mere malformation
/// so callers (and users) can tell a corrupt download from an attack.
pub fn verify_manifest(
    bytes: &[u8],
    trust_root: &VerifyingKey,
) -> Result<ReleaseManifest, ManifestError> {
    let envelope: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| ManifestError::Malformed(error.to_string()))?;
    let signature_value = envelope
        .get("signature")
        .ok_or(ManifestError::BadSignature("missing signature field"))?
        .clone();

    let mut signed_object = match envelope {
        serde_json::Value::Object(map) => map,
        _ => {
            return Err(ManifestError::Malformed(
                "manifest must be an object".into(),
            ));
        }
    };
    if signed_object.remove("signature").is_none() {
        return Err(ManifestError::BadSignature("missing signature field"));
    }

    let algorithm = signature_value
        .get("algorithm")
        .and_then(|value| value.as_str())
        .ok_or(ManifestError::BadSignature("signature has no algorithm"))?;
    if algorithm != SIGNATURE_ALGORITHM {
        return Err(ManifestError::BadSignature("unknown signature algorithm"));
    }
    let encoded_signature = signature_value
        .get("value")
        .and_then(|value| value.as_str())
        .ok_or(ManifestError::BadSignature("signature has no value"))?;
    let raw_signature = BASE64_STANDARD
        .decode(encoded_signature)
        .map_err(|_| ManifestError::BadSignature("signature is not valid base64"))?;
    let signature = Signature::from_slice(&raw_signature)
        .map_err(|_| ManifestError::BadSignature("signature has the wrong length"))?;

    // serde_json maps are BTreeMaps, so this serialization is canonical
    // (sorted keys) regardless of how the signer ordered them.
    let canonical = serde_json::to_vec(&serde_json::Value::Object(signed_object))
        .map_err(|error| ManifestError::Malformed(error.to_string()))?;
    trust_root.verify(&canonical, &signature).map_err(|_| {
        ManifestError::BadSignature("signature does not verify under the pinned release key")
    })?;

    let manifest: ReleaseManifest = serde_json::from_slice(&canonical)
        .map_err(|error| ManifestError::Malformed(error.to_string()))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// Signs `manifest` and returns the complete envelope bytes. Used by
/// tooling and tests; production signing happens in the release pipeline,
/// never on user machines.
pub fn sign_manifest(
    manifest: &ReleaseManifest,
    signing_key: &SigningKey,
) -> Result<Vec<u8>, ManifestError> {
    validate_manifest(manifest)?;
    let payload = serde_json::to_value(manifest)
        .map_err(|error| ManifestError::Malformed(error.to_string()))?;
    let serde_json::Value::Object(mut object) = payload else {
        return Err(ManifestError::Malformed(
            "manifest must serialize to an object".into(),
        ));
    };
    let canonical = serde_json::to_vec(&serde_json::Value::Object(object.clone()))
        .map_err(|error| ManifestError::Malformed(error.to_string()))?;
    let signature = signing_key.sign(&canonical);
    object.insert(
        "signature".to_owned(),
        serde_json::json!({
            "algorithm": SIGNATURE_ALGORITHM,
            "value": BASE64_STANDARD.encode(signature.to_bytes()),
        }),
    );
    serde_json::to_vec_pretty(&serde_json::Value::Object(object))
        .map_err(|error| ManifestError::Malformed(error.to_string()))
}

fn validate_manifest(manifest: &ReleaseManifest) -> Result<(), ManifestError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(ManifestError::InvalidContent(
            "unsupported schema version; upgrade the CLI to read newer releases",
        ));
    }
    if manifest.release.version.trim().is_empty() {
        return Err(ManifestError::InvalidContent("release version is empty"));
    }
    let artifact = &manifest.release.artifact;
    if artifact.file_name.is_empty()
        || artifact.file_name.contains(['/', '\\'])
        || artifact.file_name == ".."
        || artifact.file_name.contains('\0')
    {
        return Err(ManifestError::InvalidContent(
            "artifact file name must be a bare file name",
        ));
    }
    if artifact.sha256.len() != 64 || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ManifestError::InvalidContent(
            "artifact sha256 must be 64 hex characters",
        ));
    }
    if artifact.size_bytes == 0 {
        return Err(ManifestError::InvalidContent(
            "artifact size must be positive",
        ));
    }
    Ok(())
}

/// Decodes the manifest's hex digest into raw bytes for comparison.
pub fn artifact_sha256_bytes(manifest: &ReleaseManifest) -> Result<[u8; 32], ManifestError> {
    let hex = manifest.release.artifact.sha256.to_ascii_lowercase();
    let mut out = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = (chunk[0] as char).to_digit(16).unwrap_or(16) as u8;
        let low = (chunk[1] as char).to_digit(16).unwrap_or(16) as u8;
        if high > 15 || low > 15 {
            return Err(ManifestError::InvalidContent(
                "artifact sha256 must be 64 hex characters",
            ));
        }
        out[index] = (high << 4) | low;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            release: Release {
                version: "0.1.0".to_owned(),
                artifact: Artifact {
                    file_name: "lazarus-hostd".to_owned(),
                    size_bytes: 128,
                    sha256: "a".repeat(64),
                },
            },
        }
    }

    fn keys() -> (SigningKey, VerifyingKey) {
        let signer = SigningKey::from_bytes(&[7u8; 32]);
        let verifier = signer.verifying_key();
        (signer, verifier)
    }

    #[test]
    fn signed_manifests_round_trip() {
        let (signer, verifier) = keys();
        let envelope = sign_manifest(&sample(), &signer).expect("signs");
        let verified = verify_manifest(&envelope, &verifier).expect("verifies");
        assert_eq!(verified, sample());
    }

    #[test]
    fn tampered_payloads_or_keys_fail_signature_verification() {
        let (signer, verifier) = keys();
        let other_signer = SigningKey::from_bytes(&[9u8; 32]);

        let envelope = sign_manifest(&sample(), &signer).expect("signs");
        let error = verify_manifest(&envelope, &other_verifying()).unwrap_err();
        assert!(matches!(error, ManifestError::BadSignature(_)), "{error:?}");

        let wrong_key = sign_manifest(&sample(), &other_signer).expect("signs");
        assert!(matches!(
            verify_manifest(&wrong_key, &verifier).unwrap_err(),
            ManifestError::BadSignature(_)
        ));

        // One flipped byte inside the signed content invalidates it.
        let text = String::from_utf8(envelope).expect("utf8");
        let flipped = text.replacen("\"0.1.0\"", "\"0.1.1\"", 1);
        let error = verify_manifest(flipped.as_bytes(), &verifier).unwrap_err();
        assert!(matches!(error, ManifestError::BadSignature(_)), "{error:?}");
    }

    fn other_verifying() -> VerifyingKey {
        SigningKey::from_bytes(&[9u8; 32]).verifying_key()
    }

    #[test]
    fn malformed_envelopes_are_distinct_from_tampering() {
        let (_, verifier) = keys();
        for raw in [
            &b"not json"[..],
            b"{}".as_slice(),
            br#"{"schemaVersion":1}"#,
            br#"{"schemaVersion":1,"release":{},"signature":{"algorithm":"rsa","value":"AA"}}"#,
            br#"{"schemaVersion":1,"release":{},"signature":{"algorithm":"ed25519","value":"!!!"}}"#,
        ] {
            match verify_manifest(raw, &verifier).unwrap_err() {
                ManifestError::Malformed(_) | ManifestError::BadSignature(_) => {}
                other => panic!("unexpected classification for {raw:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn invalid_content_is_refused_before_anything_is_signed_or_trusted() {
        let (signer, _) = keys();
        let bad_manifests = vec![
            ReleaseManifest {
                schema_version: 99,
                ..sample()
            },
            ReleaseManifest {
                release: Release {
                    version: "  ".to_owned(),
                    ..sample().release
                },
                ..sample()
            },
            ReleaseManifest {
                release: Release {
                    artifact: Artifact {
                        file_name: "../escape".to_owned(),
                        ..sample().release.artifact
                    },
                    ..sample().release
                },
                ..sample()
            },
            ReleaseManifest {
                release: Release {
                    artifact: Artifact {
                        file_name: "dir/escape".to_owned(),
                        ..sample().release.artifact
                    },
                    ..sample().release
                },
                ..sample()
            },
            ReleaseManifest {
                release: Release {
                    artifact: Artifact {
                        sha256: "zz".to_owned(),
                        ..sample().release.artifact
                    },
                    ..sample().release
                },
                ..sample()
            },
            ReleaseManifest {
                release: Release {
                    artifact: Artifact {
                        size_bytes: 0,
                        ..sample().release.artifact
                    },
                    ..sample().release
                },
                ..sample()
            },
        ];
        for manifest in bad_manifests {
            let error = match (sign_manifest(&manifest, &signer), ()) {
                (Err(error), ()) => error,
                (Ok(_), ()) => panic!("invalid manifest was signed: {manifest:?}"),
            };
            assert!(
                matches!(error, ManifestError::InvalidContent(_)),
                "expected InvalidContent for {manifest:?}: {error:?}"
            );
        }
    }
}
