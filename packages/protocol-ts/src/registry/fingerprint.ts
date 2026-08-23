import { createHash } from 'node:crypto';

import { z } from 'zod';

/**
 * Deterministic fingerprinting for the contract registry.
 *
 * JSON Schemas are rendered by Zod, then serialized through a canonical
 * form (object keys sorted recursively) before hashing, so the same
 * contract always yields the same SHA-256 digest regardless of key order.
 * These fingerprints are the cross-language anchor: the Rust bindings must
 * carry byte-identical values (see `scripts/generate-protocol-bindings.mjs`
 * and `crates/protocol-rs/src/generated_registry.rs`).
 *
 * Stability rules:
 * - JSON Schemas render with `io: 'input'` and `reused: 'inline'` so reused
 *   subschemas never introduce `$ref` indirection into the digest;
 * - any change to these rendering options is a fingerprint-breaking event
 *   and must be released as a coordinated contract regeneration.
 */

/** Stable, deterministic JSON serialization (recursively sorted keys). */
export function canonicalJson(value: unknown): string {
  return JSON.stringify(canonicalize(value));
}

function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value !== null && typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>)
      .map(([key, entry]) => [key, canonicalize(entry)] as const)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return Object.fromEntries(entries);
  }
  return value;
}

/** Input-side JSON Schema for a Zod schema (defaults resolved, optional fields not required). */
export function jsonSchemaOf(schema: z.ZodType): unknown {
  return z.toJSONSchema(schema, { io: 'input', reused: 'inline' });
}

export function sha256Hex(text: string): string {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

export function fingerprintSchema(schema: z.ZodType): string {
  return sha256Hex(canonicalJson(jsonSchemaOf(schema)));
}

export function isFingerprint(value: string): boolean {
  return /^[0-9a-f]{64}$/.test(value);
}
