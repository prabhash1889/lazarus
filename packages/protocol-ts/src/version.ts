/**
 * Wire protocol version this package speaks. Must stay in sync with
 * `handshake::PROTOCOL_MAJOR/MINOR` in crates/protocol-rs.
 */
export const PROTOCOL_VERSION = { major: 1, minor: 0 } as const;

/** gRPC metadata header carrying client-chosen idempotency keys for write RPCs. */
export const IDEMPOTENCY_KEY_HEADER = 'x-lazarus-idempotency-key';
