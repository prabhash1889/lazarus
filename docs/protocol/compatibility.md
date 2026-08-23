# Protocol Compatibility Rules

The Lazarus Protocol connects Desktop and CLI clients to the Host, and the Host to `lazarus-runnerd`. These rules let components upgrade independently within declared ranges.

## Versioning model

- **Version format:** `major.minor` for the wire protocol (plus component versions carried separately in handshake).
- **Major change:** incompatible wire behavior. Clients and Hosts with different major versions must fail clearly, never silently misbehave.
- **Minor change:** additive fields/methods only. Backward-compatible within a major version.
- Every RPC declares its minimum required protocol minor.
- Unknown fields are ignored by receivers; removed field numbers/identifiers are never reused.

## Handshake contract

Every connection starts with a versioned handshake:

1. client sends its identity, protocol `{major, minor}`, supported features, and local auth token;
2. Host replies with its versions, negotiated minor = min(client minor, host minor) within the same major, and capability map;
3. mismatched majors are rejected with an explicit, actionable error;
4. capabilities not offered by the Host must degrade gracefully or be refused explicitly - clients probe before use.

## Component boundaries

Each boundary versions independently:

| Boundary          | Mechanism                                                                        |
| ----------------- | -------------------------------------------------------------------------------- |
| Desktop <-> Host  | Lazarus Protocol handshake                                                       |
| CLI <-> Host      | Lazarus Protocol handshake                                                       |
| Host <-> Runner   | Same protocol family + drain/reconcile semantics                                 |
| Persistence       | SQLite schema migrations (`maximum_schema_version`), separate from wire versions |
| Artifacts/exports | `artifact_format_version` in every export                                        |
| Provider packs    | `adapter_version` + declared compatibility range per pack                        |

A wire-protocol bump never implies a storage migration, and vice versa.

## Compatibility guarantees

- Within a major version: old-minor clients work against new-minor Hosts; new-minor clients work against old-minor Hosts except for features they must feature-detect via capabilities.
- Across major versions: refuse with clear error; no best-effort interpretation.
- Streaming: per-stream ordered sequences, reconnect from last acknowledged sequence, bounded replay window, explicit `STREAM_GAP` when replay is impossible.
- Write RPCs accept idempotency keys; duplicate keys never duplicate mutations.
- Runner upgrades that would kill incompatible live processes require drain first.

## Testing requirements

CI must cover at minimum:

1. old-minor client against new Host;
2. unsupported major fails with a clear error;
3. unknown additive field tolerated end-to-end;
4. duplicate idempotency key does not duplicate mutation;
5. reconnect/resume across a Host restart;
6. golden fixtures for each provider's structured protocol output.

## Change process

Breaking changes require: a superseding ADR or protocol RFC note, a major version bump, a documented migration path, and compatibility tests updated before merge.
