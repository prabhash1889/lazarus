import type {
  CompatibilityViolation,
  MethodDefinition,
  MethodSupport,
  MethodVersion,
} from './types.ts';
import { METHODS } from './methods.ts';

/**
 * One declarative step of a version bridge. Steps are plain data so the
 * same declaration is executable in TypeScript and code-generated into the
 * Rust bindings; nothing here may express arbitrary computation.
 */
export type BridgeStep =
  /** Removes named top-level response fields added after the older minor. */
  { readonly op: 'omitResponseFields'; readonly fields: readonly string[] };

/**
 * Explicit upgrade/downgrade bridges.
 *
 * When a method moves forward within a major version, a bridge adapts
 * traffic between the older minor (`older`) this Host keeps interoperable
 * and the current minor (`newer`). `steps` translate a `newer`-minor payload
 * down to `older` shape; the reverse direction needs no translation while
 * minors stay strictly additive (peers ignore unknown fields). Bridges are
 * data only - both languages execute the identical declaration, and both
 * directions are tested before release.
 */
export interface MethodBridge {
  readonly method: string;
  /** The older peer minor this bridge keeps interoperable. */
  readonly older: MethodVersion;
  /** The newer minor whose payloads the steps translate down. */
  readonly newer: MethodVersion;
  readonly steps: readonly BridgeStep[];
}

const bridgeKey = (method: string, olderMinor: number): string => `${method}:${olderMinor}`;

/** Registry of declared bridges, keyed by method + older minor. */
export const BRIDGES = new Map<string, MethodBridge>();

export function declareBridge(bridge: MethodBridge): void {
  if (bridge.older.major !== bridge.newer.major) {
    throw new Error('bridges only exist within one method major version');
  }
  if (bridge.older.minor >= bridge.newer.minor) {
    throw new Error(
      `bridge for ${bridge.method} must target a strictly older minor than ${bridge.newer.minor}`,
    );
  }
  BRIDGES.set(bridgeKey(bridge.method, bridge.older.minor), bridge);
}

/** Looks up the declared bridge keeping `older` interoperable with `method`. */
export function findBridge(method: string, olderMinor: number): MethodBridge | undefined {
  return BRIDGES.get(bridgeKey(method, olderMinor));
}

/**
 * Executes a bridge's steps on a `newer`-minor payload, returning the
 * `older`-minor rendering. This is the single TypeScript executor for every
 * declared bridge; it must stay in lockstep with the generated Rust
 * executor over the same declarative steps.
 */
export function adaptNewerToOlder(bridge: MethodBridge, payload: unknown): unknown {
  let value = payload;
  for (const step of bridge.steps) {
    if (step.op === 'omitResponseFields') {
      if (value === null || typeof value !== 'object' || Array.isArray(value)) continue;
      const object = { ...(value as Record<string, unknown>) };
      for (const field of step.fields) delete object[field];
      value = object;
    }
  }
  return value;
}

/**
 * Resolves how `method` interoperates with a peer that speaks
 * `peerVersion`, given our local definition:
 * - peer at or above our minor within the major: directly supported by the
 *   additive-minor guarantee (we simply do not send fields it lacks);
 * - peer below our minor: supported only when a declared bridge connects
 *   the two minors, so richer semantics survive the translation;
 * - otherwise not interoperable for this method.
 */
export function bridgeFor(
  method: MethodDefinition,
  peerVersion: MethodVersion,
): MethodBridge | undefined {
  const local = method.version;
  if (peerVersion.major !== local.major) return undefined;
  if (peerVersion.minor >= local.minor) return undefined;
  return findBridge(method.name, peerVersion.minor);
}

/** True when a peer's version is usable for this method, with or without a bridge. */
export function isInteroperable(method: MethodDefinition, peerVersion: MethodVersion): boolean {
  if (peerVersion.major !== method.version.major) return false;
  if (peerVersion.minor >= method.version.minor) return true;
  return bridgeFor(method, peerVersion) !== undefined;
}

/**
 * Release gate for bridge coverage (paired with the frozen-contract schema
 * check in `validation.ts`): whenever a *required* released method's minor
 * advanced, a declared bridge from the released minor to the current version
 * must exist. Rust negotiation refuses any older peer minor without a
 * generated bridge entry (`isInteroperable`), so letting generation pass
 * without one would 412 the released client at runtime even though every
 * schema change was additive.
 *
 * Optional methods are exempt by design: an unsupported optional method
 * degrades to its fallback or to `unsupported`
 * (`resolveMethodSupport`) instead of failing negotiation.
 *
 * A declared bridge must also target exactly the current registry version -
 * otherwise its executable steps would translate payloads of a minor we no
 * longer serve. `assertRegistryInvariants` enforces the same rule over the
 * live registry; this gate re-checks it per transition so neither path can
 * silently skip it.
 */
export function requiredBridgeCoverageViolations(
  released: readonly { readonly name: string; readonly version: MethodVersion }[],
  current: readonly MethodDefinition[],
): CompatibilityViolation[] {
  const violations: CompatibilityViolation[] = [];
  const currentByName = new Map(current.map((method) => [method.name, method]));
  for (const releasedMethod of released) {
    const method = currentByName.get(releasedMethod.name);
    // Removal and major changes are reported by the frozen-contract check.
    if (
      method === undefined ||
      method.optional ||
      method.version.major !== releasedMethod.version.major ||
      method.version.minor <= releasedMethod.version.minor
    ) {
      continue;
    }
    const bridge = findBridge(method.name, releasedMethod.version.minor);
    const targetsCurrent =
      bridge !== undefined &&
      bridge.newer.major === method.version.major &&
      bridge.newer.minor === method.version.minor;
    if (!targetsCurrent) {
      violations.push({
        method: method.name,
        rule: 'bridge-coverage-required',
        detail:
          `${method.name}: required method's minor advanced ` +
          `${releasedMethod.version.major}.${releasedMethod.version.minor} -> ` +
          `${method.version.major}.${method.version.minor} without a declared bridge from ` +
          `the released minor to the current version; released peers would be rejected at negotiation`,
      });
    }
  }
  return violations;
}

/**
 * Older peer minors (strictly below this method's own minor) that remain
 * interoperable through a declared bridge, sorted ascending.
 *
 * This is the codegen-facing view of the bridge registry: the generated
 * Rust negotiator accepts an older peer minor exactly when it appears here,
 * mirroring `isInteroperable`. Newer peers never appear - they are covered
 * by the additive-minor guarantee without a bridge.
 */
export function bridgedOlderMinors(method: MethodDefinition): readonly number[] {
  const minors = new Set<number>();
  for (const bridge of BRIDGES.values()) {
    if (bridge.method !== method.name) continue;
    if (
      bridge.newer.major === method.version.major &&
      bridge.newer.minor === method.version.minor &&
      bridge.older.major === bridge.newer.major &&
      bridge.older.minor < bridge.newer.minor
    ) {
      minors.add(bridge.older.minor);
    }
  }
  return [...minors].sort((a, b) => a - b);
}

export function resolveMethodSupport(
  method: MethodDefinition,
  peerVersions: ReadonlyMap<string, MethodVersion>,
): MethodSupport {
  const peerVersion = peerVersions.get(method.name);
  if (peerVersion !== undefined && isInteroperable(method, peerVersion)) {
    return 'supported';
  }
  if (method.optional && method.fallback !== undefined) {
    const fallbackMethod = METHODS.find((candidate) => candidate.name === method.fallback);
    const fallbackVersion = peerVersions.get(method.fallback);
    if (
      fallbackMethod !== undefined &&
      fallbackVersion !== undefined &&
      isInteroperable(fallbackMethod, fallbackVersion)
    ) {
      return 'fallback';
    }
  }
  return 'unsupported';
}
