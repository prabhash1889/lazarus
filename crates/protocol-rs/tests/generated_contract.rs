//! Cross-language fingerprint equality: the generated Rust contract
//! registry must match the TypeScript/Zod manifest byte for byte. The
//! golden JSON is emitted by the same generator run that produced
//! `src/generated_registry.rs`, so any drift fails here.

use protocol_rs::generated_registry::wire::{
    ERROR_ENVELOPE_FINGERPRINT, ERROR_ENVELOPE_MAJOR, ERROR_ENVELOPE_MINOR,
};
use protocol_rs::generated_registry::{
    BridgeStep, MANIFEST_FINGERPRINT, METHOD_BINDINGS, METHOD_BRIDGES, binding_by_name,
};

#[test]
fn generated_registry_matches_typescript_manifest() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/protocol_manifest.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let golden: serde_json::Value = serde_json::from_str(&raw).expect("golden manifest parses");

    assert_eq!(
        golden["manifestFingerprint"].as_str().expect("fingerprint"),
        MANIFEST_FINGERPRINT,
        "manifest fingerprint drifted from the TypeScript registry"
    );

    let methods = golden["methods"].as_array().expect("methods array");
    assert_eq!(methods.len(), METHOD_BINDINGS.len(), "method count drifted");
    for entry in methods {
        let name = entry["name"].as_str().expect("method name");
        let binding = binding_by_name(name).unwrap_or_else(|| panic!("missing binding for {name}"));
        assert_eq!(
            binding.kind.as_str(),
            entry["kind"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            binding.major,
            entry["major"].as_u64().unwrap() as u32,
            "{name}"
        );
        assert_eq!(
            binding.minor,
            entry["minor"].as_u64().unwrap() as u32,
            "{name}"
        );
        assert_eq!(
            binding.optional,
            entry["optional"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            binding.fallback,
            entry["fallback"].as_str(),
            "{name} fallback"
        );
        assert_eq!(
            binding.request_fingerprint,
            entry["requestFingerprint"].as_str().unwrap(),
            "{name} request fingerprint"
        );
        assert_eq!(
            binding.response_fingerprint,
            entry["responseFingerprint"].as_str().unwrap(),
            "{name} response fingerprint"
        );
    }

    let floor = golden["releasedFloor"].as_array().expect("floor array");
    let rust_floor: Vec<&str> = protocol_rs::generated_registry::RELEASED_FLOOR.to_vec();
    let json_floor: Vec<&str> = floor
        .iter()
        .map(|value| value.as_str().expect("floor name"))
        .collect();
    assert_eq!(rust_floor, json_floor, "released floor drifted");

    let error = &golden["errorEnvelope"];
    assert_eq!(
        error["major"].as_u64().unwrap() as u32,
        ERROR_ENVELOPE_MAJOR
    );
    assert_eq!(
        error["minor"].as_u64().unwrap() as u32,
        ERROR_ENVELOPE_MINOR
    );
    assert_eq!(
        error["fingerprint"].as_str().unwrap(),
        ERROR_ENVELOPE_FINGERPRINT,
        "error envelope fingerprint drifted"
    );
}

#[test]
fn generated_bridges_match_typescript_declarations() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/protocol_manifest.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let golden: serde_json::Value = serde_json::from_str(&raw).expect("golden manifest parses");

    let bridges = golden["bridges"].as_array().expect("bridges array");
    assert_eq!(
        bridges.len(),
        METHOD_BRIDGES.len(),
        "declared bridge count drifted"
    );
    for entry in bridges {
        let name = entry["name"].as_str().expect("bridge method name");
        let older_minor = entry["olderMinor"].as_u64().expect("older minor") as u32;
        let newer_minor = entry["newerMinor"].as_u64().expect("newer minor") as u32;
        // The bridge must target the binding's current minor.
        let binding = binding_by_name(name).unwrap_or_else(|| panic!("missing binding {name}"));
        assert_eq!(
            newer_minor, binding.minor,
            "{name} bridge targets a stale minor"
        );

        let steps = protocol_rs::generated_registry::bridge_steps(name, older_minor);
        let expected_steps: Vec<serde_json::Value> = steps
            .iter()
            .map(|step| match step {
                BridgeStep::OmitResponseFields(fields) => serde_json::json!({
                    "op": "omitResponseFields",
                    "fields": fields,
                }),
            })
            .collect();
        assert_eq!(
            serde_json::Value::Array(expected_steps),
            entry["steps"],
            "{name}@{older_minor} steps drifted from the TypeScript declaration"
        );
    }
}
