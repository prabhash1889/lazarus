import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';

import { z } from 'zod';

import {
  ERROR_ENVELOPE,
  ERROR_ENVELOPE_NAME,
  ERROR_ENVELOPE_VERSION,
  errorEnvelopeFingerprint,
  isFingerprint,
  isRetryableErrorCode,
  jsonSchemaOf,
  protocolError,
  RETRYABLE_ERROR_CODES,
  validateFrozenErrorEnvelopeTransition,
} from './index.ts';
import { ErrorCodeSchema, ProtocolErrorSchema } from './schemas/common.ts';
import type { FrozenErrorEnvelope } from './index.ts';

const frozenEnvelope = (): FrozenErrorEnvelope => ({
  name: ERROR_ENVELOPE_NAME,
  version: ERROR_ENVELOPE_VERSION,
  requestSchema: {},
  responseSchema: jsonSchemaOf(ERROR_ENVELOPE.schema),
});

describe('canonical retryable error classification', () => {
  it('marks exactly DEADLINE_EXCEEDED and UNAVAILABLE retryable', () => {
    assert.deepEqual([...RETRYABLE_ERROR_CODES].sort(), ['DEADLINE_EXCEEDED', 'UNAVAILABLE']);
  });

  it('classifies every declared code and nothing else', () => {
    for (const code of ErrorCodeSchema.options) {
      assert.equal(typeof isRetryableErrorCode(code), 'boolean', code);
    }
    assert.equal(isRetryableErrorCode('UNAVAILABLE'), true);
    assert.equal(isRetryableErrorCode('DEADLINE_EXCEEDED'), true);
    for (const terminal of ErrorCodeSchema.options.filter(
      (code) => code !== 'UNAVAILABLE' && code !== 'DEADLINE_EXCEEDED',
    )) {
      assert.equal(isRetryableErrorCode(terminal), false, terminal);
    }
  });

  it('CANCELLED stays terminal: an automatic retry would contradict the caller', () => {
    assert.equal(isRetryableErrorCode('CANCELLED'), false);
  });
});

describe('versioned error envelope contract', () => {
  it('carries the explicit retryability label on the wire', () => {
    const parsed = ProtocolErrorSchema.parse(protocolError('UNAVAILABLE', 'host is restarting'));
    assert.deepEqual(parsed, {
      code: 'UNAVAILABLE',
      message: 'host is restarting',
      retryable: true,
    });

    const terminal = ProtocolErrorSchema.parse(
      protocolError('INCOMPATIBLE_METHOD_MANIFEST', 'major mismatch'),
    );
    assert.equal(terminal.retryable, false);

    // The label is mandatory: a body without it fails to decode, as does an
    // unknown code.
    assert.equal(ProtocolErrorSchema.safeParse({ code: 'INTERNAL', message: 'x' }).success, false);
    assert.equal(ProtocolErrorSchema.safeParse({ code: 'WHOOPS', message: 'x' }).success, false);
  });

  it('is fingerprinted like a method payload', () => {
    assert.ok(isFingerprint(errorEnvelopeFingerprint()));
    assert.equal(errorEnvelopeFingerprint(), errorEnvelopeFingerprint());
  });
});

describe('frozen error-envelope gate', () => {
  it('accepts the current envelope against itself', () => {
    assert.deepEqual(validateFrozenErrorEnvelopeTransition(frozenEnvelope(), frozenEnvelope()), []);
  });

  it('rejects a same-version edit of the wire shape', () => {
    const edited = ProtocolErrorSchema.extend({ hint: z.string().optional() });
    const candidate: FrozenErrorEnvelope = {
      ...frozenEnvelope(),
      responseSchema: jsonSchemaOf(edited),
    };
    const violations = validateFrozenErrorEnvelopeTransition(frozenEnvelope(), candidate);
    assert.equal(violations.length, 1);
    assert.equal(violations[0]!.rule, 'minor-bump-required');
  });

  it('rejects narrowing the shape but accepts an additive-minor widening', () => {
    // Dropping the retryable label breaks every released client.
    const narrowed = ProtocolErrorSchema.pick({ code: true, message: true });
    const breaking: FrozenErrorEnvelope = {
      ...frozenEnvelope(),
      version: { major: 1, minor: 1 },
      responseSchema: jsonSchemaOf(narrowed),
    };
    const violations = validateFrozenErrorEnvelopeTransition(frozenEnvelope(), breaking);
    assert.equal(violations.length, 1);
    assert.equal(violations[0]!.rule, 'breaking-schema-change');

    // An added optional field released as a minor bump stays additive.
    const widened = ProtocolErrorSchema.extend({ hint: z.string().optional() });
    const additive: FrozenErrorEnvelope = {
      ...frozenEnvelope(),
      version: { major: 1, minor: 1 },
      responseSchema: jsonSchemaOf(widened),
    };
    assert.deepEqual(validateFrozenErrorEnvelopeTransition(frozenEnvelope(), additive), []);
  });

  it('rejects a major change on the envelope like any method payload', () => {
    const candidate: FrozenErrorEnvelope = {
      ...frozenEnvelope(),
      version: { major: 2, minor: 0 },
    };
    const violations = validateFrozenErrorEnvelopeTransition(frozenEnvelope(), candidate);
    assert.equal(violations.length, 1);
    assert.equal(violations[0]!.rule, 'major-changed');
  });
});
