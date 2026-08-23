//! Executable version bridges.
//!
//! The generated [`crate::generated_registry::METHOD_BRIDGES`] table carries
//! the declarative steps declared in the TypeScript/Zod source of truth; this
//! module is the single Rust executor for them. It must stay in lockstep
//! with `adaptNewerToOlder` in the protocol package: same declaration,
//! identical result on both sides of the wire.

use crate::generated_registry::{self, BridgeStep};
use serde_json::Value;

/// Applies bridge steps to a response payload in place. Steps only ever
/// remove top-level fields, so a non-object payload is left untouched.
pub fn apply_bridge_steps(payload: &mut Value, steps: &[BridgeStep]) {
    for step in steps {
        match step {
            BridgeStep::OmitResponseFields(fields) => {
                if let Some(object) = payload.as_object_mut() {
                    for field in *fields {
                        object.remove(*field);
                    }
                }
            }
        }
    }
}

/// The steps needed to serve `name` to a peer negotiated at
/// `peer_minor`: empty unless the peer sits strictly below the host's
/// minor at an older minor a declared bridge keeps interoperable.
pub fn downgrade_response_steps(name: &str, peer_minor: u32) -> &'static [BridgeStep] {
    let Some(binding) = generated_registry::binding_by_name(name) else {
        return &[];
    };
    if peer_minor >= binding.minor {
        return &[];
    }
    generated_registry::bridge_steps(name, peer_minor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn omit_response_fields_strips_only_top_level_fields() {
        let mut payload = json!({
            "tasks": [{"id": "t-1", "extra": true}],
            "pagination": {"nextCursor": null},
            "servedAtUnixMs": 42,
        });
        apply_bridge_steps(
            &mut payload,
            &[BridgeStep::OmitResponseFields(&[
                "servedAtUnixMs",
                "absent",
            ])],
        );
        assert_eq!(
            payload,
            json!({
                "tasks": [{"id": "t-1", "extra": true}],
                "pagination": {"nextCursor": null},
            })
        );

        // Non-object payloads pass through untouched.
        let mut array = json!([{"servedAtUnixMs": 1}]);
        apply_bridge_steps(
            &mut array,
            &[BridgeStep::OmitResponseFields(&["servedAtUnixMs"])],
        );
        assert_eq!(array, json!([{"servedAtUnixMs": 1}]));
    }

    #[test]
    fn downgrade_steps_only_apply_to_bridged_older_minors() {
        // The declared 1.0 bridge for task.list (host serves 1.2).
        assert_eq!(
            downgrade_response_steps("task.list", 0),
            generated_registry::bridge_steps("task.list", 0)
        );
        assert!(!downgrade_response_steps("task.list", 0).is_empty());

        // Minor 1 was never published: numerically plausible but undeclared.
        assert!(downgrade_response_steps("task.list", 1).is_empty());
        // Current and newer peers need no adaptation.
        assert!(downgrade_response_steps("task.list", 2).is_empty());
        assert!(downgrade_response_steps("task.list", 9).is_empty());
        // Methods without any declared bridge never adapt.
        assert!(downgrade_response_steps("workspace.list", 0).is_empty());
        assert!(downgrade_response_steps("no.such_method", 0).is_empty());
    }
}
