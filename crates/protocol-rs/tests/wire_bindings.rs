//! Cross-language payload equality: every canonical fixture was rendered
//! and Zod-validated by the TypeScript registry; the generated Rust wire
//! decoders must accept exactly these instances, tolerate unknown additive
//! fields, and enforce the schema's constraints.

use protocol_rs::generated_registry::wire;

const FIXTURES_RAW: &str = include_str!("wire_fixtures.json");

fn fixtures() -> serde_json::Value {
    serde_json::from_str(FIXTURES_RAW).expect("fixtures parse")
}

fn method_fixture(name: &str, role: &str) -> serde_json::Value {
    fixtures()["methods"][name][role].clone()
}

/// Compile-enforced completeness: adding a method to the registry without a
/// decoder arm here fails this test build.
#[test]
fn generated_decoders_accept_every_typescript_validated_fixture() {
    let fixtures = fixtures();
    let methods = fixtures["methods"].as_object().expect("methods map");
    assert_eq!(methods.len(), 10);

    for (name, entry) in methods {
        // Both decoders are erased to Result<(), _> so every arm shares one
        // type while keeping the full error display for failures.
        let outcome = match name.as_str() {
            "process.list" => (
                wire::decode_process_list_request(&entry["request"]).map(|_| ()),
                wire::decode_process_list_response(&entry["response"]).map(|_| ()),
            ),
            "process.output" => (
                wire::decode_process_output_request(&entry["request"]).map(|_| ()),
                wire::decode_process_output_response(&entry["response"]).map(|_| ()),
            ),
            "process.resume" => (
                wire::decode_process_resume_request(&entry["request"]).map(|_| ()),
                wire::decode_process_resume_response(&entry["response"]).map(|_| ()),
            ),
            "process.start" => (
                wire::decode_process_start_request(&entry["request"]).map(|_| ()),
                wire::decode_process_start_response(&entry["response"]).map(|_| ()),
            ),
            "process.stop" => (
                wire::decode_process_stop_request(&entry["request"]).map(|_| ()),
                wire::decode_process_stop_response(&entry["response"]).map(|_| ()),
            ),
            "system.getInfo" => (
                wire::decode_system_get_info_request(&entry["request"]).map(|_| ()),
                wire::decode_system_get_info_response(&entry["response"]).map(|_| ()),
            ),
            "system.health" => (
                wire::decode_system_health_request(&entry["request"]).map(|_| ()),
                wire::decode_system_health_response(&entry["response"]).map(|_| ()),
            ),
            "system.subscribeEvents" => (
                wire::decode_system_subscribe_events_request(&entry["request"]).map(|_| ()),
                wire::decode_system_subscribe_events_response(&entry["response"]).map(|_| ()),
            ),
            "task.list" => (
                wire::decode_task_list_request(&entry["request"]).map(|_| ()),
                wire::decode_task_list_response(&entry["response"]).map(|_| ()),
            ),
            "workspace.list" => (
                wire::decode_workspace_list_request(&entry["request"]).map(|_| ()),
                wire::decode_workspace_list_response(&entry["response"]).map(|_| ()),
            ),
            other => panic!("fixture for unknown method {other}; extend the decoder arms"),
        };
        assert!(
            outcome.0.is_ok(),
            "{name} request fixture must decode: {:?}",
            outcome.0
        );
        assert!(
            outcome.1.is_ok(),
            "{name} response fixture must decode: {:?}",
            outcome.1
        );
    }

    // The shared error envelope decodes with its canonical retryability.
    let envelope = wire::decode_protocol_error(&fixtures["errorEnvelope"]).expect("envelope");
    assert_eq!(envelope.code.as_str(), "UNAVAILABLE");
    assert!(envelope.retryable);
}

/// Unknown additive fields pass through untouched - the additive-minor
/// guarantee at the payload boundary.
#[test]
fn decoders_tolerate_unknown_additive_fields() {
    let mut additive = method_fixture("task.list", "response");
    additive["futureField"] = serde_json::json!({"anything": [1, 2, 3]});
    let decoded = wire::decode_task_list_response(&additive).expect("additive tolerated");
    assert!(decoded.tasks.is_empty());

    let mut additive = method_fixture("system.health", "response");
    additive["newStatusDetail"] = serde_json::json!("extra context");
    assert!(wire::decode_system_health_response(&additive).is_ok());
}

/// Constraints from the contract schemas are enforced on top of decoding:
/// bounds, required fields, and types all reject violating payloads.
#[test]
fn decoders_enforce_schema_constraints() {
    let invalid_process_id = serde_json::json!({
        "processId": "not-a-uuid-v7",
        "program": "git",
        "args": [],
        "runMode": "PIPED",
        "dataDir": "D:/tmp/lazarus",
    });
    let error = wire::decode_process_start_request(&invalid_process_id)
        .expect_err("processId must be UUIDv7");
    assert!(error.to_string().contains("processId"), "{error}");

    // pageSize below/above the contracted bounds fails validation...
    for bad in [0u64, 101] {
        let payload = serde_json::json!({ "pageSize": bad });
        let error = wire::decode_task_list_request(&payload).expect_err("bound must hold");
        assert!(
            error.to_string().contains("pageSize"),
            "{error} names the field"
        );
    }
    // ...and in-bound values decode cleanly.
    assert!(
        wire::decode_task_list_request(&serde_json::json!({"pageSize": 100, "cursor": "abc"}))
            .is_ok()
    );

    // Required fields cannot be omitted...
    let missing_status = wire::decode_system_health_response(&serde_json::json!({}));
    assert!(missing_status.is_err(), "required fields are mandatory");

    // ...and wrong types fail decoding rather than coercing.
    let wrong_type = wire::decode_system_get_info_response(&serde_json::json!({
        "hostVersion": "lazarus",
        "capabilities": {"lazarus": "not-a-boolean"},
    }));
    assert!(wrong_type.is_err(), "wrong value types must fail decoding");

    // The outage frame's non-empty id is a schema constraint.
    let empty_id = serde_json::json!({"type": "outage", "outageId": ""});
    let error = wire::decode_system_subscribe_events_response(&empty_id)
        .expect_err("empty outageId violates minLength");
    assert!(error.to_string().contains("outageId"), "{error}");
}
