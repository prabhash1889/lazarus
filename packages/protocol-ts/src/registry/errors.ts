import type { MethodVersion } from './types.ts';

import { ErrorCodeSchema, ProtocolErrorSchema } from './schemas/common.ts';
import { fingerprintSchema } from './fingerprint.ts';
import { z } from 'zod';

/** A canonical protocol error code, as carried by the wire envelope. */
export type ProtocolErrorCode = z.infer<typeof ErrorCodeSchema>;

/** Every declared error code, sorted; codegen embeds this set verbatim. */
export const ERROR_CODE_OPTIONS: readonly ProtocolErrorCode[] = [...ErrorCodeSchema.options].sort();

/**
 * The canonical error envelope as a versioned contract artifact.
 *
 * It is not an RPC method, so it cannot ride the per-method manifest;
 * instead it is generated into the Rust bindings and validated against the
 * frozen released-contract baseline (`released-contract.json`) exactly like
 * a method payload, so error wire-shape changes can never bypass the
 * release gates.
 */
export const ERROR_ENVELOPE_NAME = 'protocol.error' as const;

export const ERROR_ENVELOPE_VERSION: MethodVersion = { major: 1, minor: 0 };

export interface ErrorEnvelopeDefinition {
  readonly name: typeof ERROR_ENVELOPE_NAME;
  readonly version: MethodVersion;
  readonly schema: z.ZodType;
}

export const ERROR_ENVELOPE: ErrorEnvelopeDefinition = {
  name: ERROR_ENVELOPE_NAME,
  version: ERROR_ENVELOPE_VERSION,
  schema: ProtocolErrorSchema,
};

/** SHA-256 of the canonical JSON Schema of the error envelope. */
export function errorEnvelopeFingerprint(): string {
  return fingerprintSchema(ERROR_ENVELOPE.schema);
}

/**
 * Error codes the canonical classification marks retryable: the failure is
 * transient and re-issuing the same call (idempotency-keyed where it
 * writes) may succeed. Every other code is terminal: retrying cannot help,
 * so callers must stop, report, or change something first.
 *
 * - `UNAVAILABLE`: Host/transport temporarily unreachable or overloaded.
 * - `DEADLINE_EXCEEDED`: the caller's deadline elapsed before completion.
 *
 * `CANCELLED` is deliberately terminal: the caller withdrew the request, so
 * an automatic retry would contradict the cancellation.
 */
export const RETRYABLE_ERROR_CODES: ReadonlySet<ProtocolErrorCode> = new Set([
  'DEADLINE_EXCEEDED',
  'UNAVAILABLE',
]);

/** The canonical classification shared by TS and the generated Rust side. */
export function isRetryableErrorCode(code: ProtocolErrorCode): boolean {
  return RETRYABLE_ERROR_CODES.has(code);
}

/**
 * Builds a conforming error envelope value: `retryable` always comes from
 * the canonical classification so call sites cannot mislabel an error.
 */
export function protocolError(
  code: ProtocolErrorCode,
  message: string,
): z.infer<typeof ProtocolErrorSchema> {
  return { code, message, retryable: isRetryableErrorCode(code) };
}
