# Protocol Compatibility Rules

The Lazarus Protocol connects Desktop and CLI clients to the local Host (`lazarus-hostd`). These rules let components upgrade independently within declared ranges.

## Versioning model

- **Version format:** per-method `{major, minor}` versions. There is no single global protocol version and no lockstep negotiated minor.
- **Major change:** incompatible behavior for that method. Clients and Hosts with different major versions for a required method must fail clearly, never silently misbehave.
- **Minor change:** additive fields only, backward-compatible within a method major version. Unknown fields are ignored.
- **Contract source of truth:** the TypeScript/Zod protocol package. JSON Schema fingerprints and Rust bindings are generated from it; hand-maintained parallel contracts (e.g., Protobuf) are forbidden.

## Current Phase 1.5 transport

Phase 1.5 uses loopback-only Axum JSON/HTTP. This is the current local implementation, not a permanent 1.0 transport commitment.

| Method                   | Transport                 |
| ------------------------ | ------------------------- |
| `system.getInfo`         | `GET /system/info` JSON   |
| `system.health`          | `GET /system/health` JSON |
| `workspace.list`         | `GET /workspaces` JSON    |
| `task.list`              | `GET /tasks` JSON         |
| `system.subscribeEvents` | `GET /system/events` SSE  |

Host, CLI, and Desktop read the per-install `LAZARUS_LOCAL_TOKEN`. Every request carries `Authorization: Bearer <token>`, the client's complete per-method manifest in `x-lazarus-manifest`, and its cancellation deadline in `x-lazarus-deadline`; every successful response returns the Host manifest in the same manifest header. Authentication happens before manifest parsing or handler logic.

Compatibility is decided **per method**: the major must match and the negotiated minor is the lower supported minor. An incompatible required method returns HTTP 412 with the typed JSON code `INCOMPATIBLE_METHOD_MANIFEST`. Optional methods declare a named fallback or `unsupported`; one unsupported optional method never fails unrelated RPCs.

## Wire payloads and generated bindings

The TypeScript/Zod registry is the single source of truth for payload shapes. The generator (`scripts/generate-protocol-bindings.mjs`) emits, from the same schemas that produce the fingerprints:

1. **Rust wire types** (`protocol_rs::generated_registry::wire`) for every method's request and response payloads, with serde derives matching the JSON shapes exactly;
2. **generated decode validation**: decoders enforce required fields, reject wrong types, tolerate unknown additive fields (the additive-minor guarantee at the payload boundary), and run schema-derived constraint checks (bounds, lengths) via each type's `validate()`;
3. **canonical fixtures** (`crates/protocol-rs/tests/wire_fixtures.json`): sample payloads rendered and Zod-validated by the TypeScript registry; CI proves the Rust decoders accept exactly these instances, so Host and client payload drift fails a test instead of a user session.

The Host builds responses from the generated types; the CLI and Desktop decode every response body through them before use. Anything the generator cannot render from a schema fails generation loudly - it never emits a lossy type.

## Error envelope and retryability

Every typed error body is the versioned `protocol.error` envelope:

```json
{ "code": "DEADLINE_EXCEEDED", "message": "...", "retryable": true }
```

It is frozen in the released-contract baseline like every method payload: versioned (`{major, minor}`), fingerprinted, generated into the Rust bindings, and gated by the same additive-minor rules, so error-shape changes can never bypass the release gates.

The canonical retryability classification lives once, in the TypeScript registry (`RETRYABLE_ERROR_CODES`), and is code-generated into the Rust side:

| Classification | Codes                                  | Caller behavior                                                                                                                                                      |
| -------------- | -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Retryable      | `UNAVAILABLE`, `DEADLINE_EXCEEDED`     | Re-issuing the call (idempotency-keyed where it writes) may succeed.                                                                                                 |
| Terminal       | everything else, including `CANCELLED` | Retrying cannot help; stop, report, or change something first. `CANCELLED` stays terminal because an automatic retry would contradict the caller's own cancellation. |

Call sites cannot mislabel errors: the Host constructs envelopes through `ProtocolError::new`, which derives `retryable` from the classification. Clients surface the label ("(retryable)") so operators can distinguish transient failures from dead ends.

## Deadlines and cancellation

Cancellation/deadline semantics are transport-neutral and ride one header:

- `x-lazarus-deadline`: absolute Unix epoch milliseconds by which the request must finish.

Rules:

1. A Host stops working when the deadline passes: the handler future is dropped mid-flight and the canonical typed `DEADLINE_EXCEEDED` (HTTP 504) answers instead. An already-elapsed deadline is rejected immediately with the same error.
2. Closing the connection cancels whatever the deadline does not cover.
3. A malformed header is a typed `INVALID_ARGUMENT` (400), never silently ignored, so callers cannot believe a budget exists when it does not.
4. Streaming subscriptions close at the deadline too; clients resubscribe for a fresh snapshot, consistent with the recovery model below.
5. CLI and Desktop stamp deadlines from the shared default budget (`DEFAULT_RPC_BUDGET_MS`, 5 s) and time their own transports out at the same point plus a small receive grace, so the Host's typed rejection wins the race and both clients behave identically.

## Component boundaries

Each boundary versions independently:

| Boundary          | Mechanism                                                                                   |
| ----------------- | ------------------------------------------------------------------------------------------- |
| Desktop <-> Host  | Lazarus Protocol per-method manifests                                                       |
| CLI <-> Host      | Lazarus Protocol per-method manifests                                                       |
| Persistence       | Per-record `{major, minor}` versions + SQLite schema migrations, separate from RPC versions |
| Artifacts/exports | `artifact_format_version` in every export                                                   |
| Provider packs    | `adapter_version` + declared compatibility range per pack                                   |

Package semver is distribution metadata only - never handshake currency. The SQLite migration number is never the RPC version. A wire-protocol bump never implies a storage migration, and vice versa.

## Compatibility guarantees

- Within a method major version: old-minor clients work against new-minor Hosts via additive-minor validation and explicit upgrade/downgrade bridges; new-minor clients degrade gracefully against older Hosts through fallbacks/`unsupported`.
- Across majors: refuse with clear error; no best-effort interpretation.
- CI enforces a frozen released-floor method-name set every supported Host still serves, structural additivity of minors via JSON Schema fingerprints, executable bridge tests in both directions, and fingerprint equality between TypeScript schemas and generated Rust bindings - for method payloads and the error envelope alike.
- Write RPCs accept idempotency keys where needed; duplicate keys never duplicate mutations.

## Recovery semantics

Recovery does not use a universal replay envelope, reconnect token, replay window, or `STREAM_GAP` contract:

1. `/system/events` sends a restart tombstone carrying the current outage id unless the client supplies the same id in `x-lazarus-last-outage-id`;
2. every subscription then receives an authoritative snapshot before live events;
3. a lagged stream closes, and the client resubscribes to obtain a fresh snapshot.

Retryable errors are explicitly labeled by the canonical classification (see above); everything else is terminal.

## Testing requirements

CI must cover at minimum:

1. old and new per-method versions interoperating through declared bridges;
2. a breaking minor schema failing fingerprint/additivity validation;
3. unsupported major for a required method fails with a clear error;
4. unknown additive field tolerated end-to-end (generated decoders and live Host traffic);
5. every released-floor method present on both sides;
6. an optional unsupported method degrading without failing unrelated RPCs;
7. restart tombstone deduplicates one outage and clients restore state by resubscribing + snapshot;
8. unauthenticated requests rejected; non-loopback local bind configuration rejected;
9. golden fixtures for each provider's structured protocol output;
10. the TypeScript-rendered wire fixtures decode through every generated Rust decoder;
11. retryable and terminal classifications agree across the TS registry and the generated Rust side;
12. an elapsed deadline stops work and answers typed `DEADLINE_EXCEEDED`; malformed deadlines fail typed.

## Change process

Breaking changes require: a superseding ADR or protocol RFC note, a major version bump on the affected methods, a documented migration path, updated bridges/floor set, and compatibility tests updated before merge.
