# Lazarus Protocol Schemas

Protobuf definitions for the Lazarus Protocol wire version 1.x
(`LAZARUS_INITIAL_PLAN.md` section 9). Package: `lazarus.protocol.v1`.

## Layout

| File              | Contents                                                                        |
| ----------------- | ------------------------------------------------------------------------------- |
| `common.proto`    | Envelope, error model, pagination, reconnect token                              |
| `handshake.proto` | Client handshake and host reply (capability negotiation)                        |
| `system.proto`    | `SystemService`: Negotiate, GetInfo, Health, SubscribeEvents (server streaming) |
| `workspace.proto` | `WorkspaceService`: List                                                        |
| `task.proto`      | `TaskService`: List                                                             |

Later phases add more files (`agent`, `terminal`, `git`, `artifact`,
`workflow`, ...) - never reuse field numbers; additive minor changes only.

## Contract decisions

### Handshake and negotiation

`System.Negotiate(ClientHello) returns (HostReply)` is an explicit unary RPC
rather than a transport-level first frame: tonic clients cannot inject custom
pre-RPC framing, and an explicit RPC keeps negotiation testable and usable
from both Desktop and CLI.

Rules:

- major versions must match exactly; a mismatch is rejected with gRPC
  `FAILED_PRECONDITION` whose details encode
  `Error { code: ERROR_CODE_UNSUPPORTED_PROTOCOL_VERSION }`;
- `negotiated_minor = min(client.minor, host.minor)` - older minor clients
  interoperate with newer minor hosts and vice versa;
- capabilities AND-combine: enabled only when both sides advertise them.

Reference implementation plus compatibility tests live in
`crates/protocol-rs/src/handshake.rs` and
`crates/protocol-rs/tests/contract.rs`. The Rust constants are
`handshake::PROTOCOL_MAJOR/MINOR`; the TypeScript mirror is
`PROTOCOL_VERSION` in `packages/protocol-ts/src/version.ts`.

### Cancellation

Cancellation is transport-level only. A client cancels any RPC (including the
`SubscribeEvents` server stream) by cancelling/dropping the call; tonic
propagates that to the host by dropping its response stream. There is no
cancel RPC in the schema and none is needed - this is covered by the
`dropping_streaming_rpc_cancels_on_the_server` contract test.

### Reconnect and resume

`SubscribeEventsRequest.reconnect` carries a `ReconnectToken`; hosts resume
the event stream from its acknowledged sequence or answer with
`ERROR_CODE_STREAM_GAP` when replay is impossible.

### Idempotency

Write RPCs carry a client-chosen idempotency key in gRPC metadata under the
`x-lazarus-idempotency-key` header. Keys are deliberately metadata, not
message fields, so every future write RPC inherits the convention additively.
Hosts must replay the original response for a repeated key instead of
executing the mutation again. Helpers and reference semantics:
`crates/protocol-rs/src/idempotency.rs`.

### Pagination

List RPCs use cursor pagination (`PaginationRequest.page_token` /
`PaginationResponse.next_page_token`). Tokens are opaque strings; invalid or
out-of-range tokens fail with `INVALID_ARGUMENT`.

## Regenerating bindings

Both generators run from the repo root:

- TypeScript: `pnpm gen:protocol` (writes to `packages/protocol-ts/src/gen/`)
- Rust: `cargo build -p protocol-rs` (the crate's build script compiles the
  same protos via prost/tonic into `OUT_DIR`; nothing is checked in)

Buf lint/breaking checks: `pnpm exec buf lint proto`.

The buf CLI comes from the root devDependency `@bufbuild/buf`; no global
install is required.
