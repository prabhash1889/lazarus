import type * as z from 'zod';

/**
 * Per-method semantic version. Methods evolve independently: a minor bump
 * must stay strictly additive (see `validation.ts`); a major bump breaks
 * compatibility and requires a declared bridge or a peer upgrade.
 */
export interface MethodVersion {
  readonly major: number;
  readonly minor: number;
}

/** Transport shape of the method: single response or a server-side stream. */
export type MethodKind = 'unary' | 'serverStreaming';

/**
 * How a caller treats a method the peer did not advertise:
 * - `supported`: peer implements it at a compatible version;
 * - `fallback`: optional method with a local substitute, degrade gracefully;
 * - `unsupported`: optional method without a substitute, hide/disable.
 */
export type MethodSupport = 'supported' | 'fallback' | 'unsupported';

/** One RPC method in the protocol contract, described by Zod schemas. */
export interface MethodDefinition {
  /** Fully qualified name, `<service>.<method>` (e.g. `system.health`). */
  readonly name: string;
  readonly kind: MethodKind;
  readonly version: MethodVersion;
  readonly request: z.ZodType;
  readonly response: z.ZodType;
  /**
   * Optional methods may be absent from a peer without failing unrelated
   * RPCs; they degrade to a fallback (when one exists) or to `unsupported`.
   */
  readonly optional: boolean;
  /**
   * Name of the substitute method used when this optional method is not
   * supported by the peer. Required for optional methods; must name another
   * method in the registry.
   */
  readonly fallback?: string;
}

/** A plain-data rendering of a [`MethodDefinition`] used for diffing and codegen. */
export interface MethodSnapshot {
  readonly name: string;
  readonly kind: MethodKind;
  readonly version: MethodVersion;
  readonly optional: boolean;
  readonly fallback?: string;
  readonly requestFingerprint: string;
  readonly responseFingerprint: string;
}

/** Plain-data rendering of a full manifest (the codegen interchange form). */
export interface ManifestSnapshot {
  readonly methods: readonly MethodSnapshot[];
  readonly manifestFingerprint: string;
}

/** Why a candidate manifest cannot replace the previously released one. */
export interface CompatibilityViolation {
  readonly method: string;
  readonly rule:
    | 'major-changed'
    | 'method-removed'
    | 'minor-regressed'
    | 'minor-bump-required'
    | 'breaking-schema-change'
    | 'bridge-coverage-required';
  readonly detail: string;
}
