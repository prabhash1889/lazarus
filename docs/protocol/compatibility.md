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

Host, CLI, and Desktop read the per-install `LAZARUS_LOCAL_TOKEN`. Every request carries `Authorization: Bearer <token>` and the client's complete per-method manifest in `x-lazarus-manifest`; every successful response returns the Host manifest in the same header. Authentication happens before manifest parsing or handler logic.

Compatibility is decided **per method**: the major must match and the negotiated minor is the lower supported minor. An incompatible required method returns HTTP 412 with the typed JSON code `INCOMPATIBLE_METHOD_MANIFEST`. Optional methods declare a named fallback or `unsupported`; one unsupported optional method never fails unrelated RPCs.

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
- CI enforces a frozen released-floor method-name set every supported Host still serves, structural additivity of minors via JSON Schema fingerprints, executable bridge tests in both directions, and fingerprint equality between TypeScript schemas and generated Rust bindings.
- Write RPCs accept idempotency keys where needed; duplicate keys never duplicate mutations.

## Recovery semantics

Recovery does not use a universal replay envelope, reconnect token, replay window, or `STREAM_GAP` contract:

1. `/system/events` sends a restart tombstone carrying the current outage id unless the client supplies the same id in `x-lazarus-last-outage-id`;
2. every subscription then receives an authoritative snapshot before live events;
3. a lagged stream closes, and the client resubscribes to obtain a fresh snapshot.

Retryable errors remain explicitly labeled.

## Testing requirements

CI must cover at minimum:

1. old and new per-method versions interoperating through declared bridges;
2. a breaking minor schema failing fingerprint/additivity validation;
3. unsupported major for a required method fails with a clear error;
4. unknown additive field tolerated end-to-end;
5. every released-floor method present on both sides;
6. an optional unsupported method degrading without failing unrelated RPCs;
7. restart tombstone deduplicates one outage and clients restore state by resubscribing + snapshot;
8. unauthenticated requests rejected; non-loopback local bind configuration rejected;
9. golden fixtures for each provider's structured protocol output.

## Change process

Breaking changes require: a superseding ADR or protocol RFC note, a major version bump on the affected methods, a documented migration path, updated bridges/floor set, and compatibility tests updated before merge.
