import { isRetryableErrorCode, type ProtocolErrorCode } from '@lazarus/protocol-ts';

/**
 * Typed failures of a Host protocol call. Every branch carries the
 * canonical error code so callers (and the UI) can classify without string
 * matching; `retryable` always comes from the canonical classification.
 */
export class ProtocolCallError extends Error {
  readonly code: ProtocolErrorCode | 'TRANSPORT';
  /** True only when the canonical classification marks the code retryable. */
  readonly retryable: boolean;
  /** Which layer failed: the transport bridge, or the Host's typed answer. */
  readonly layer: 'transport' | 'host';

  constructor(init: {
    code: ProtocolErrorCode | 'TRANSPORT';
    message: string;
    layer: 'transport' | 'host';
  }) {
    super(init.message);
    this.name = 'ProtocolCallError';
    this.code = init.code;
    this.layer = init.layer;
    this.retryable =
      init.code === 'TRANSPORT'
        ? true
        : isRetryableErrorCode(init.code satisfies ProtocolErrorCode);
  }
}

/** The caller aborted an in-flight unary call. Terminal by definition. */
export class RequestCancelledError extends Error {
  constructor() {
    super('the request was cancelled');
    this.name = 'RequestCancelledError';
  }
}

/**
 * The Host's advertised manifest cannot serve this client. Names every
 * offending method; terminal until either side upgrades.
 */
export class IncompatibleManifestError extends Error {
  readonly offenders: string[];

  constructor(offenders: string[]) {
    const rendered =
      offenders.length > 0
        ? `incompatible method manifest for ${offenders.join(', ')}`
        : 'the host advertised no usable methods';
    super(rendered);
    this.name = 'IncompatibleManifestError';
    this.offenders = offenders;
  }
}

/** Type guard narrowing any thrown value to a protocol call error. */
export function isProtocolCallError(error: unknown): error is ProtocolCallError {
  return error instanceof ProtocolCallError;
}
