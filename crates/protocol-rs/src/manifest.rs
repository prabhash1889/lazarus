//! Per-method manifest compatibility.
//!
//! The generated [`crate::generated_registry`] describes every protocol
//! method with its own semantic version. Peers advertise which methods they
//! serve through a *manifest*: the complete list of method names plus the
//! major/minor version each is served at.
//!
//! The wire form of a manifest is a single compact ASCII string suitable for
//! one request header value:
//!
//! ```text
//! v1:system.getInfo=1.0,system.health=1.0,...
//! ```
//!
//! Entries are sorted by name, so encoding is deterministic for a given
//! method set. Names may contain only ASCII alphanumerics, `.`, and `_`,
//! which keeps the format unambiguous (`=`, `,`, and `:` are structural).
//!
//! Negotiation rules, applied per method:
//! - majors must match exactly;
//! - a peer at or above the host minor is supported at the host minor (the
//!   additive-minor guarantee);
//! - a peer below the host minor is supported at its own minor only when the
//!   generated bridge table ([`crate::generated_registry::BRIDGED_PEER_MINORS`],
//!   derived from the declared TypeScript bridges) declares that minor
//!   interoperable;
//! - an optional method failing the above degrades to its registered
//!   fallback, but only when the peer itself serves that fallback at an
//!   interoperable major/minor; unsupported when it does not;
//! - a required method failing the above is a typed violation, collected
//!   alongside every other violation so unrelated compatible methods are
//!   never implicated or masked.

use crate::generated_registry::{self, METHOD_BINDINGS};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

/// Upper bound for an encoded manifest, comfortably inside typical request
/// header value limits. Decoding anything larger is rejected outright.
pub const MAX_MANIFEST_LEN: usize = 4096;

/// Request header carrying the encoded [`MethodManifest`] on requests
/// (the peer's own manifest) and on responses (this side's complete
/// manifest).
pub const MANIFEST_METADATA_KEY: &str = "x-lazarus-manifest";

const FORMAT_PREFIX: &str = "v1:";
const MAX_METHOD_NAME_LEN: usize = 128;

/// Per-method version advertised in a manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodVersion {
    pub major: u32,
    pub minor: u32,
}

/// A parsed manifest: method name to served version, kept sorted by name.
///
/// Duplicate names are rejected on construction, and iteration order is the
/// sorted order used when encoding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MethodManifest {
    methods: BTreeMap<String, MethodVersion>,
}

impl MethodManifest {
    /// Inserts a method version, rejecting a duplicate method name.
    pub fn try_insert(
        &mut self,
        name: impl Into<String>,
        major: u32,
        minor: u32,
    ) -> Result<(), ManifestDecodeError> {
        let name = name.into();
        validate_name(&name)?;
        if self
            .methods
            .insert(name.clone(), MethodVersion { major, minor })
            .is_some()
        {
            return Err(ManifestDecodeError::DuplicateMethod { name });
        }
        Ok(())
    }

    /// The version a method is advertised at, if present.
    pub fn get(&self, name: &str) -> Option<MethodVersion> {
        self.methods.get(name).copied()
    }

    /// Sorted iteration over `(name, version)` entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, MethodVersion)> {
        self.methods.iter().map(|(name, version)| (name, *version))
    }

    pub fn len(&self) -> usize {
        self.methods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }
}

impl fmt::Display for MethodManifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(FORMAT_PREFIX)?;
        for (index, (name, version)) in self.methods.iter().enumerate() {
            if index > 0 {
                f.write_str(",")?;
            }
            write!(f, "{name}={}.{}", version.major, version.minor)?;
        }
        Ok(())
    }
}

impl std::str::FromStr for MethodManifest {
    type Err = ManifestDecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() > MAX_MANIFEST_LEN {
            return Err(ManifestDecodeError::TooLarge { len: s.len() });
        }
        if s.is_empty() {
            return Err(ManifestDecodeError::Empty);
        }
        let body = s
            .strip_prefix(FORMAT_PREFIX)
            .ok_or(ManifestDecodeError::UnknownFormat)?;

        let mut methods = BTreeMap::new();
        for entry in body.split(',') {
            let Some((name, version)) = entry.split_once('=') else {
                return Err(ManifestDecodeError::MalformedEntry {
                    entry: entry.to_owned(),
                });
            };
            validate_name(name)?;
            let version =
                parse_version(version).ok_or_else(|| ManifestDecodeError::MalformedEntry {
                    entry: entry.to_owned(),
                })?;
            if methods.insert(name.to_owned(), version).is_some() {
                return Err(ManifestDecodeError::DuplicateMethod {
                    name: name.to_owned(),
                });
            }
        }
        Ok(Self { methods })
    }
}

/// Why an encoded manifest could not be decoded (or built).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestDecodeError {
    Empty,
    TooLarge { len: usize },
    UnknownFormat,
    MalformedEntry { entry: String },
    InvalidMethodName { name: String },
    DuplicateMethod { name: String },
}

impl fmt::Display for ManifestDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestDecodeError::Empty => f.write_str("manifest is empty"),
            ManifestDecodeError::TooLarge { len } => {
                write!(
                    f,
                    "manifest is {len} bytes, above the {MAX_MANIFEST_LEN} byte limit"
                )
            }
            ManifestDecodeError::UnknownFormat => {
                write!(f, "manifest must start with {FORMAT_PREFIX:?}")
            }
            ManifestDecodeError::MalformedEntry { entry } => {
                write!(
                    f,
                    "malformed manifest entry {entry:?}, expected <name>=<major>.<minor>"
                )
            }
            ManifestDecodeError::InvalidMethodName { name } => {
                write!(f, "invalid method name {name:?}")
            }
            ManifestDecodeError::DuplicateMethod { name } => {
                write!(f, "duplicate method name {name:?}")
            }
        }
    }
}

impl std::error::Error for ManifestDecodeError {}

fn validate_name(name: &str) -> Result<(), ManifestDecodeError> {
    if name.is_empty() || name.len() > MAX_METHOD_NAME_LEN {
        return Err(ManifestDecodeError::InvalidMethodName {
            name: name.to_owned(),
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        return Err(ManifestDecodeError::InvalidMethodName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn parse_version(s: &str) -> Option<MethodVersion> {
    let (major, minor) = s.split_once('.')?;
    let major = parse_u32(major)?;
    let minor = parse_u32(minor)?;
    Some(MethodVersion { major, minor })
}

fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The manifest of the local host, built from the generated bindings.
pub fn host_manifest() -> MethodManifest {
    let mut manifest = MethodManifest::default();
    for binding in METHOD_BINDINGS {
        manifest
            .try_insert(binding.name, binding.major, binding.minor)
            .expect("generated bindings have unique valid names");
    }
    manifest
}

/// The encoded [`host_manifest`], computed once and reused for every
/// response that must advertise it.
pub fn host_manifest_encoded() -> &'static str {
    static ENCODED: OnceLock<String> = OnceLock::new();
    ENCODED.get_or_init(|| host_manifest().to_string())
}

/// One method's outcome after negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Both sides serve the method; the negotiated minor is agreed.
    Supported { minor: u32 },
    /// Optional method the peer cannot serve; the registered fallback
    /// substitute is named. Emitted only when the peer also serves the
    /// fallback at an interoperable version.
    Fallback { fallback: &'static str },
    /// Optional method the peer cannot serve and no fallback exists.
    Unsupported,
}

/// Why a required method cannot be negotiated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incompatibility {
    RequiredMissing {
        name: String,
    },
    MajorMismatch {
        name: String,
        host_major: u32,
        peer_major: u32,
    },
    /// Peer serves an older minor with no declared bridge to the host minor.
    UndeclaredMinor {
        name: String,
        host_minor: u32,
        peer_minor: u32,
    },
}

impl fmt::Display for Incompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Incompatibility::RequiredMissing { name } => {
                write!(f, "required method {name:?} missing from peer manifest")
            }
            Incompatibility::MajorMismatch {
                name,
                host_major,
                peer_major,
            } => {
                write!(
                    f,
                    "method {name:?} major mismatch: host serves {host_major}.x, peer serves {peer_major}.x"
                )
            }
            Incompatibility::UndeclaredMinor {
                name,
                host_minor,
                peer_minor,
            } => {
                write!(
                    f,
                    "method {name:?} peer minor {peer_minor} predates host minor {host_minor} and no bridge is declared"
                )
            }
        }
    }
}

/// Every required-method violation found while negotiating. Carries all
/// violations rather than the first, so one broken method neither hides
/// another nor implicates any compatible method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibleManifest {
    pub violations: Vec<Incompatibility>,
}

impl fmt::Display for IncompatibleManifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, violation) in self.violations.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{violation}")?;
        }
        Ok(())
    }
}

impl std::error::Error for IncompatibleManifest {}

/// Per-method results of a successful negotiation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NegotiatedManifest {
    /// Every host method resolved independently, sorted by name: supported
    /// at a common version, degraded to a fallback, or unsupported.
    pub methods: Vec<(String, Resolution)>,
}

/// Negotiates a peer manifest against a host manifest, method by method.
///
/// Every method in the host manifest is resolved independently: majors must
/// match; newer peers clamp down to the host minor (additive-minor
/// guarantee); older peers are accepted only at a minor the generated bridge
/// table declares interoperable. Optional methods the peer cannot serve
/// degrade to their registered fallback (looked up in
/// [`crate::generated_registry`]) when the peer serves that fallback too, or
/// to unsupported otherwise; a required method the
/// peer cannot serve produces a typed violation. All violations are returned
/// together, naming exactly the offending methods and nothing else.
pub fn negotiate(
    host: &MethodManifest,
    peer: &MethodManifest,
) -> Result<NegotiatedManifest, IncompatibleManifest> {
    negotiate_with_bridges(host, peer, generated_registry::BRIDGED_PEER_MINORS)
}

/// Like [`negotiate`], but with an explicit bridge table instead of the
/// generated one. The table has the same shape as
/// [`crate::generated_registry::BRIDGED_PEER_MINORS`]: `(method name, older
/// peer minors declared interoperable)` pairs. Private so the public surface
/// always negotiates against the TypeScript-derived bridge table; kept as a
/// seam for unit tests.
fn negotiate_with_bridges(
    host: &MethodManifest,
    peer: &MethodManifest,
    bridged_minors: &[(&str, &[u32])],
) -> Result<NegotiatedManifest, IncompatibleManifest> {
    let mut methods = Vec::new();
    let mut violations = Vec::new();

    for (name, host_version) in host.iter() {
        let binding = generated_registry::binding_by_name(name);
        let optional = binding.is_some_and(|b| b.optional);
        let fallback = binding.and_then(|b| b.fallback);
        let bridged = bridged_minors
            .iter()
            .find(|(binding_name, _)| *binding_name == name)
            .map(|(_, minors)| *minors)
            .unwrap_or(&[]);

        match resolve_method(
            name,
            host_version,
            peer.get(name),
            bridged,
            optional,
            fallback,
        ) {
            Ok(Some(Resolution::Fallback { fallback })) => {
                let resolution = fallback_resolution(fallback, peer, bridged_minors);
                methods.push((name.clone(), resolution));
            }
            Ok(Some(resolution)) => methods.push((name.clone(), resolution)),
            Ok(None) => {}
            Err(violation) => violations.push(violation),
        }
    }

    if violations.is_empty() {
        Ok(NegotiatedManifest { methods })
    } else {
        Err(IncompatibleManifest { violations })
    }
}

/// Convenience form negotiating against [`host_manifest`].
pub fn negotiate_with_host(
    peer: &MethodManifest,
) -> Result<NegotiatedManifest, IncompatibleManifest> {
    negotiate(&host_manifest(), peer)
}

fn resolve_method(
    name: &str,
    host_version: MethodVersion,
    peer_version: Option<MethodVersion>,
    bridged_minors: &[u32],
    optional: bool,
    fallback: Option<&'static str>,
) -> Result<Option<Resolution>, Incompatibility> {
    let agreed_minor = peer_version
        .and_then(|peer_version| agreed_minor(host_version, peer_version, bridged_minors));
    if let Some(minor) = agreed_minor {
        return Ok(Some(Resolution::Supported { minor }));
    }

    match (optional, peer_version) {
        // The caller confirms the peer actually serves the fallback before
        // the degradation is finalized.
        (true, _) => Ok(Some(match fallback {
            Some(fallback) => Resolution::Fallback { fallback },
            None => Resolution::Unsupported,
        })),
        (false, None) => Err(Incompatibility::RequiredMissing {
            name: name.to_owned(),
        }),
        (false, Some(peer)) if peer.major != host_version.major => {
            Err(Incompatibility::MajorMismatch {
                name: name.to_owned(),
                host_major: host_version.major,
                peer_major: peer.major,
            })
        }
        (false, Some(peer)) => Err(Incompatibility::UndeclaredMinor {
            name: name.to_owned(),
            host_minor: host_version.minor,
            peer_minor: peer.minor,
        }),
    }
}

/// The minor two sides can talk at, applying the negotiation policy: majors
/// must match exactly; a newer peer clamps down to the host minor
/// (additive-minor guarantee); an older peer is accepted only at a minor a
/// declared bridge keeps interoperable.
fn agreed_minor(
    host_version: MethodVersion,
    peer_version: MethodVersion,
    bridged_minors: &[u32],
) -> Option<u32> {
    if peer_version.major != host_version.major {
        None
    } else if peer_version.minor >= host_version.minor {
        // Additive-minor guarantee: a newer peer clamps down to ours.
        Some(host_version.minor)
    } else if bridged_minors.contains(&peer_version.minor) {
        // Declared bridge: talk at the older peer's minor.
        Some(peer_version.minor)
    } else {
        None
    }
}

/// Finalizes a degraded optional method against its registered fallback:
/// the fallback is honored only when it is itself a registered method the
/// peer advertises at a major/minor interoperable with our own binding for
/// it, under the same negotiation policy as any other method.
fn fallback_resolution(
    fallback: &'static str,
    peer: &MethodManifest,
    bridged_minors: &[(&str, &[u32])],
) -> Resolution {
    let Some(binding) = generated_registry::binding_by_name(fallback) else {
        return Resolution::Unsupported;
    };
    let host_version = MethodVersion {
        major: binding.major,
        minor: binding.minor,
    };
    let bridged = bridged_minors
        .iter()
        .find(|(name, _)| *name == fallback)
        .map(|(_, minors)| *minors)
        .unwrap_or(&[]);
    match peer
        .get(fallback)
        .and_then(|peer_version| agreed_minor(host_version, peer_version, bridged))
    {
        Some(_) => Resolution::Fallback { fallback },
        None => Resolution::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_registry::RELEASED_FLOOR;

    fn manifest(entries: &[(&str, u32, u32)]) -> MethodManifest {
        let mut manifest = MethodManifest::default();
        for &(name, major, minor) in entries {
            manifest.try_insert(name, major, minor).expect("test entry");
        }
        manifest
    }

    #[test]
    fn roundtrips_all_released_floor_methods() {
        let host = host_manifest();
        assert_eq!(host.len(), RELEASED_FLOOR.len());
        for name in RELEASED_FLOOR {
            assert!(
                host.get(name).is_some(),
                "floor method {name} in host manifest"
            );
        }

        let encoded = host.to_string();
        assert!(
            encoded.is_ascii(),
            "encoded manifest must be ASCII: {encoded}"
        );
        let decoded: MethodManifest = encoded.parse().expect("decode");
        assert_eq!(decoded, host);
    }

    #[test]
    fn encoding_is_deterministic_and_sorted() {
        let first = manifest(&[("z.method", 1, 0), ("a.method", 2, 3)]);
        let second = manifest(&[("a.method", 2, 3), ("z.method", 1, 0)]);
        assert_eq!(first.to_string(), second.to_string());
        assert_eq!(
            first.to_string(),
            "v1:a.method=2.3,z.method=1.0",
            "entries must render sorted by name"
        );
    }

    #[test]
    fn newer_peers_clamp_down_but_exact_and_newer_still_succeed() {
        let host = manifest(&[("svc.method", 1, 6)]);

        let exact_peer = manifest(&[("svc.method", 1, 6)]);
        let negotiated = negotiate(&host, &exact_peer).expect("exact version");
        assert_eq!(
            negotiated.methods,
            vec![("svc.method".to_owned(), Resolution::Supported { minor: 6 })]
        );

        let newer_peer = manifest(&[("svc.method", 1, 9)]);
        let negotiated = negotiate(&host, &newer_peer).expect("additive-minor guarantee");
        assert_eq!(
            negotiated.methods,
            vec![("svc.method".to_owned(), Resolution::Supported { minor: 6 })]
        );
    }

    #[test]
    fn undeclared_minor_mismatch_is_rejected_until_a_bridge_declares_it() {
        let host = manifest(&[("svc.method", 1, 3)]);
        let peer = manifest(&[("svc.method", 1, 1)]);

        let err = negotiate_with_bridges(&host, &peer, &[]).expect_err("older minor has no bridge");
        assert_eq!(
            err.violations,
            vec![Incompatibility::UndeclaredMinor {
                name: "svc.method".to_owned(),
                host_minor: 3,
                peer_minor: 1,
            }]
        );

        let negotiated = negotiate_with_bridges(&host, &peer, &[("svc.method", &[1])])
            .expect("declared bridge makes the older minor interoperable");
        assert_eq!(
            negotiated.methods,
            vec![("svc.method".to_owned(), Resolution::Supported { minor: 1 })]
        );
    }

    #[test]
    fn negotiate_against_host_clamps_newer_peer_minor_to_floor() {
        let mut peer = host_manifest();
        for (_, version) in peer.methods.iter_mut() {
            version.minor = 5;
        }
        let negotiated = negotiate_with_host(&peer).expect("same major");
        for (name, resolution) in &negotiated.methods {
            let expected_minor = generated_registry::binding_by_name(name)
                .expect("generated binding")
                .minor;
            assert_eq!(
                resolution,
                &Resolution::Supported {
                    minor: expected_minor
                },
                "{name} negotiates down to the host minor"
            );
        }
    }

    #[test]
    fn required_missing_and_major_mismatch_are_typed_errors() {
        let host = host_manifest();
        let peer = manifest(&[
            ("process.list", 1, 0),
            ("process.output", 1, 0),
            ("process.start", 1, 0),
            ("process.stop", 1, 0),
            ("system.getInfo", 1, 0),
            ("system.health", 1, 0),
            ("system.subscribeEvents", 1, 0),
            ("workspace.list", 2, 0),
        ]);

        let err = negotiate(&host, &peer).expect_err("task.list missing, workspace.list major");
        assert_eq!(
            err.violations,
            vec![
                Incompatibility::RequiredMissing {
                    name: "task.list".to_owned()
                },
                Incompatibility::MajorMismatch {
                    name: "workspace.list".to_owned(),
                    host_major: 1,
                    peer_major: 2
                },
            ]
        );
        let rendered = err.to_string();
        assert!(rendered.contains("workspace.list") && rendered.contains("task.list"));
        assert!(
            !rendered.contains("system.health"),
            "healthy methods stay unnamed"
        );
    }

    #[test]
    fn peer_only_methods_are_ignored() {
        let host = manifest(&[("svc.method", 1, 0)]);
        let peer = manifest(&[("svc.method", 1, 2), ("extra.method", 9, 9)]);
        let negotiated = negotiate(&host, &peer).expect("extra peers ignored");
        assert_eq!(
            negotiated.methods,
            vec![("svc.method".to_owned(), Resolution::Supported { minor: 0 })]
        );
    }

    #[test]
    fn optional_missing_resolves_fallback_or_unsupported() {
        // No current binding is optional, so exercise the decision directly:
        // a future optional method with a fallback degrades to it, and one
        // without falls to unsupported.
        let host_version = MethodVersion { major: 1, minor: 2 };

        assert_eq!(
            resolve_method(
                "opt.method",
                host_version,
                None,
                &[],
                true,
                Some("system.health")
            ),
            Ok(Some(Resolution::Fallback {
                fallback: "system.health"
            }))
        );
        assert_eq!(
            resolve_method("opt.method", host_version, None, &[], true, None),
            Ok(Some(Resolution::Unsupported))
        );

        // An incompatible major on an optional method degrades identically.
        let wrong_major = MethodVersion { major: 7, minor: 0 };
        assert_eq!(
            resolve_method(
                "opt.method",
                host_version,
                Some(wrong_major),
                &[],
                true,
                Some("system.health")
            ),
            Ok(Some(Resolution::Fallback {
                fallback: "system.health"
            }))
        );

        // Required methods never degrade: they are typed violations.
        assert_eq!(
            resolve_method("req.method", host_version, None, &[], false, None),
            Err(Incompatibility::RequiredMissing {
                name: "req.method".to_owned()
            })
        );
    }

    #[test]
    fn older_minor_without_bridge_degrades_or_violates_and_bridges_negotiate() {
        let host_version = MethodVersion { major: 1, minor: 4 };
        let older = MethodVersion { major: 1, minor: 1 };

        // An optional method facing an undeclared older minor degrades just
        // like a missing or major-mismatched one.
        assert_eq!(
            resolve_method(
                "opt.method",
                host_version,
                Some(older),
                &[],
                true,
                Some("system.health")
            ),
            Ok(Some(Resolution::Fallback {
                fallback: "system.health"
            }))
        );
        assert_eq!(
            resolve_method("opt.method", host_version, Some(older), &[], true, None),
            Ok(Some(Resolution::Unsupported))
        );

        // A required method with the same peer is a typed violation.
        assert_eq!(
            resolve_method("req.method", host_version, Some(older), &[], false, None),
            Err(Incompatibility::UndeclaredMinor {
                name: "req.method".to_owned(),
                host_minor: 4,
                peer_minor: 1,
            })
        );

        // With the bridge declared, the same peers negotiate at the older minor.
        assert_eq!(
            resolve_method("req.method", host_version, Some(older), &[1], false, None),
            Ok(Some(Resolution::Supported { minor: 1 }))
        );
    }

    #[test]
    fn fallback_is_honored_only_when_the_peer_serves_it() {
        // Peer advertises the fallback at an interoperable version: honored,
        // including at a newer minor under the additive-minor guarantee.
        assert_eq!(
            fallback_resolution("system.health", &manifest(&[("system.health", 1, 0)]), &[]),
            Resolution::Fallback {
                fallback: "system.health"
            }
        );
        assert_eq!(
            fallback_resolution("system.health", &manifest(&[("system.health", 1, 4)]), &[]),
            Resolution::Fallback {
                fallback: "system.health"
            }
        );

        // Not advertised at all: unsupported, despite the registry naming it.
        assert_eq!(
            fallback_resolution("system.health", &manifest(&[("workspace.list", 1, 0)]), &[]),
            Resolution::Unsupported
        );
        assert_eq!(
            fallback_resolution("system.health", &MethodManifest::default(), &[]),
            Resolution::Unsupported
        );

        // Advertised under a different major is not interoperable.
        assert_eq!(
            fallback_resolution("system.health", &manifest(&[("system.health", 2, 0)]), &[]),
            Resolution::Unsupported
        );

        // An older minor below the fallback's own negotiates only through a
        // declared bridge.
        let old_peer = manifest(&[("task.list", 1, 0)]);
        assert_eq!(
            fallback_resolution("task.list", &old_peer, &[]),
            Resolution::Unsupported
        );
        assert_eq!(
            fallback_resolution("task.list", &old_peer, &[("task.list", &[0])]),
            Resolution::Fallback {
                fallback: "task.list"
            }
        );
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert_eq!(
            "".parse::<MethodManifest>().unwrap_err(),
            ManifestDecodeError::Empty
        );

        let oversized = format!("v1:{}", "a=1.0,".repeat(MAX_MANIFEST_LEN));
        assert_eq!(
            oversized.parse::<MethodManifest>().unwrap_err(),
            ManifestDecodeError::TooLarge {
                len: oversized.len()
            }
        );

        assert_eq!(
            "system.getInfo=1.0".parse::<MethodManifest>().unwrap_err(),
            ManifestDecodeError::UnknownFormat
        );
        assert_eq!(
            "v2:system.getInfo=1.0"
                .parse::<MethodManifest>()
                .unwrap_err(),
            ManifestDecodeError::UnknownFormat
        );

        for bad in [
            "v1:",
            "v1:no-version",
            "v1:system.getInfo",
            "v1:system.getInfo=1",
            "v1:system.getInfo=1.",
            "v1:system.getInfo=.0",
            "v1:system.getInfo=1.0.0",
            "v1:system.getInfo=x.y",
            "v1:system.getInfo=-1.0",
            "v1:system.getInfo=1.0,",
            "v1:system getInfo=1.0",
            "v1:=1.0",
        ] {
            assert!(
                bad.parse::<MethodManifest>().is_err(),
                "must reject {bad:?}"
            );
        }

        assert_eq!(
            "v1:a.method=1.0,a.method=1.1"
                .parse::<MethodManifest>()
                .unwrap_err(),
            ManifestDecodeError::DuplicateMethod {
                name: "a.method".to_owned()
            }
        );
    }

    #[test]
    fn try_insert_rejects_duplicates() {
        let mut manifest = manifest(&[("a.method", 1, 0)]);
        assert_eq!(
            manifest.try_insert("a.method", 1, 1).unwrap_err(),
            ManifestDecodeError::DuplicateMethod {
                name: "a.method".to_owned()
            }
        );
    }

    #[test]
    fn host_manifest_encoded_is_stable_and_decodable() {
        assert_eq!(host_manifest_encoded(), host_manifest_encoded());
        let decoded: MethodManifest = host_manifest_encoded().parse().expect("decode");
        assert_eq!(decoded, host_manifest());
    }
}
