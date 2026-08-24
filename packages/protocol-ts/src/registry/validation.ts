import type { CompatibilityViolation, MethodDefinition, MethodVersion } from './types.ts';

import { jsonSchemaOf } from './fingerprint.ts';

/**
 * Additive-minor validation.
 *
 * Within one method major version, a minor bump may only *widen* the
 * contract: new optional fields, widened enums, relaxed constraints. The
 * check is structural, over the JSON Schema renderings: every input the
 * released schema accepted must still be accepted by the candidate schema.
 */

type JsonSchema = Record<string, unknown>;

/**
 * Keywords that never restrict acceptance (pure annotations); they may
 * appear or disappear freely between releases.
 */
const ANNOTATION_KEYWORDS: ReadonlySet<string> = new Set([
  '$schema',
  '$id',
  'title',
  'description',
  'default',
  'examples',
  'deprecated',
]);

/**
 * Keywords this check understands. Any other keyword on either side of the
 * comparison fails the check: an unrecognized keyword could narrow what the
 * schema accepts in ways that cannot be proven safe here.
 */
const HANDLED_KEYWORDS: ReadonlySet<string> = new Set([
  'type',
  'enum',
  'const',
  'minimum',
  'maximum',
  'minLength',
  'maxLength',
  'items',
  'properties',
  'required',
  'oneOf',
  'anyOf',
  'propertyNames',
  'additionalProperties',
]);

function unionBranches(schema: JsonSchema): readonly unknown[] | undefined {
  if ('oneOf' in schema) return schema.oneOf as readonly unknown[];
  if ('anyOf' in schema) return schema.anyOf as readonly unknown[];
  return undefined;
}

/**
 * True when `candidate` accepts at least every input `released` accepted.
 * Handles exactly the JSON Schema constructs Zod emits for this registry:
 * objects, arrays, enums, literals (`const`), unions (`oneOf`/`anyOf`,
 * including discriminated unions), records (`propertyNames` +
 * `additionalProperties`), string/number types, and bounds. Anything else
 * fails closed.
 */
export function isSchemaBackwardCompatible(released: unknown, candidate: unknown): boolean {
  return acceptsEvery(released, candidate);
}

/** True when every response the candidate may emit is accepted by released clients. */
export function isResponseSchemaBackwardCompatible(released: unknown, candidate: unknown): boolean {
  if (typeof released !== 'object' || released === null) {
    return canonicalEqual(released, candidate);
  }
  if (typeof candidate !== 'object' || candidate === null) return false;
  const old = released as JsonSchema;
  const next = candidate as JsonSchema;
  for (const side of [old, next]) {
    for (const key of Object.keys(side)) {
      if (!ANNOTATION_KEYWORDS.has(key) && !HANDLED_KEYWORDS.has(key)) return false;
    }
  }

  const oldBranches = unionBranches(old);
  const nextBranches = unionBranches(next);
  if (oldBranches !== undefined || nextBranches !== undefined) {
    if (oldBranches === undefined || nextBranches === undefined) return false;
    return (
      oldBranches.length === nextBranches.length &&
      oldBranches.every((branch) =>
        nextBranches.some((nextBranch) => isResponseSchemaBackwardCompatible(branch, nextBranch)),
      )
    );
  }

  for (const key of ['type', 'enum', 'const', 'minimum', 'maximum', 'minLength', 'maxLength']) {
    if (!canonicalEqual(old[key], next[key])) return false;
  }
  for (const key of ['items', 'propertyNames', 'additionalProperties']) {
    const oldHas = key in old;
    const nextHas = key in next;
    if (oldHas !== nextHas) return false;
    if (oldHas && !isResponseSchemaBackwardCompatible(old[key], next[key])) return false;
  }

  const oldProps = (old.properties ?? {}) as Record<string, unknown>;
  const nextProps = (next.properties ?? {}) as Record<string, unknown>;
  for (const [name, property] of Object.entries(oldProps)) {
    if (!(name in nextProps)) return false;
    if (!isResponseSchemaBackwardCompatible(property, nextProps[name])) return false;
  }
  const oldRequired = [...((old.required as readonly string[] | undefined) ?? [])].sort();
  const nextRequired = [...((next.required as readonly string[] | undefined) ?? [])].sort();
  if (!canonicalEqual(oldRequired, nextRequired)) return false;
  return Object.keys(nextProps)
    .filter((name) => !(name in oldProps))
    .every((name) => !nextRequired.includes(name));
}

/** True when `target` accepts every value admitted by `source`. */
function acceptsEvery(source: unknown, target: unknown): boolean {
  if (typeof source !== 'object' || source === null) {
    return canonicalEqual(source, target);
  }
  const old = source as JsonSchema;
  const next = target as JsonSchema | null;
  if (next === null || typeof next !== 'object') return false;

  // Fail closed on constructs outside the handled set.
  for (const side of [old, next]) {
    for (const key of Object.keys(side)) {
      if (!ANNOTATION_KEYWORDS.has(key) && !HANDLED_KEYWORDS.has(key)) return false;
    }
  }

  // Unions (including Zod's discriminated-union `oneOf`): every branch the
  // released union accepted must still be covered by some candidate branch,
  // so removing or shrinking a variant fails. When the released schema was
  // not a union, every candidate branch must itself accept everything the
  // released schema accepted.
  const oldBranches = unionBranches(old);
  const nextBranches = unionBranches(next);
  if (oldBranches !== undefined) {
    if (nextBranches === undefined) return false;
    for (const oldBranch of oldBranches) {
      if (!nextBranches.some((branch) => acceptsEvery(oldBranch, branch))) {
        return false;
      }
    }
  } else if (nextBranches !== undefined) {
    if (!nextBranches.some((branch) => acceptsEvery(old, branch))) {
      return false;
    }
  }

  // A literal is a set with one member: it must survive unchanged, and a
  // literal may never appear where the released schema had none.
  if ('const' in old) {
    if ('const' in next && !canonicalEqual(old.const, next.const)) return false;
  } else if ('const' in next) {
    return false;
  }

  // Type widening is fine; narrowing or changing is not.
  if (!typeSetIsWidened(old.type, next.type)) return false;

  // Enums may only grow, and an enum may never appear where the released
  // schema accepted the whole type: either way acceptance narrows.
  const oldEnum = old.enum as readonly unknown[] | undefined;
  const nextEnum = next.enum as readonly unknown[] | undefined;
  if (oldEnum !== undefined) {
    if (nextEnum !== undefined && !oldEnum.every((v) => nextEnum.includes(v))) {
      return false;
    }
  } else if (nextEnum !== undefined) {
    return false;
  }

  // Numeric/string bounds: a released bound may only relax (a minimum move
  // down, a maximum up), a dropped bound widens, but a newly introduced bound
  // rejects inputs the released schema accepted.
  for (const key of ['minimum', 'maximum', 'minLength', 'maxLength']) {
    if (!(key in old)) {
      if (key in next) return false;
      continue;
    }
    if (!(key in next) || canonicalEqual(old[key], next[key])) continue;
    const relaxDown = key.startsWith('min') && numLt(next[key], old[key]);
    const relaxUp = key.startsWith('max') && numLt(old[key], next[key]);
    if (!relaxDown && !relaxUp) return false;
  }

  // Arrays: the item schema itself must stay compatible, and an item schema
  // may never appear where the released schema accepted any element.
  if ('items' in old) {
    if ('items' in next && !acceptsEvery(old.items, next.items)) {
      return false;
    }
  } else if ('items' in next) {
    return false;
  }

  // Record keys: a key schema may never appear or tighten.
  if ('propertyNames' in old) {
    if ('propertyNames' in next && !acceptsEvery(old.propertyNames, next.propertyNames)) {
      return false;
    }
  } else if ('propertyNames' in next) {
    return false;
  }

  // Additional properties: a released open object must stay open; a value
  // schema (or `false`) must survive compatibly. Zod emits this for records.
  if ('additionalProperties' in old) {
    if (
      'additionalProperties' in next &&
      !acceptsEvery(old.additionalProperties, next.additionalProperties)
    ) {
      return false;
    }
  } else if ('additionalProperties' in next) {
    return false;
  }

  // Objects: every previously known property must survive compatibly and
  // the required set must never grow. New properties are free additions
  // (peers ignore unknown fields).
  const oldProps = (old.properties ?? {}) as Record<string, unknown>;
  const nextProps = (next.properties ?? {}) as Record<string, unknown>;
  for (const [name, prop] of Object.entries(oldProps)) {
    if (!(name in nextProps)) {
      return false;
    }
    if (!acceptsEvery(prop, nextProps[name])) return false;
  }
  // The required set must never grow: a newly required field rejects inputs
  // the released schema accepted.
  const oldRequired = (old.required as readonly string[] | undefined) ?? [];
  const nextRequired = (next.required as readonly string[] | undefined) ?? [];
  if (!nextRequired.every((field) => oldRequired.includes(field))) {
    return false;
  }
  return true;
}

function numLt(a: unknown, b: unknown): boolean {
  return typeof a === 'number' && typeof b === 'number' && a < b;
}

function canonicalEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(sortKeys(a)) === JSON.stringify(sortKeys(b));
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .map(([k, v]) => [k, sortKeys(v)] as const)
        .sort(([x], [y]) => (x < y ? -1 : x > y ? 1 : 0)),
    );
  }
  return value;
}

function typeSetIsWidened(releasedType: unknown, candidateType: unknown): boolean {
  if (releasedType === undefined) return candidateType === undefined;
  if (candidateType === undefined) return true;
  const released = Array.isArray(releasedType) ? releasedType : [releasedType];
  const candidate = Array.isArray(candidateType) ? candidateType : [candidateType];
  return released.every((type) => candidate.includes(type));
}

/** Version plus rendered request/response JSON Schemas for one method. */
export interface SchemaTransition {
  readonly version: MethodVersion;
  readonly requestSchema: unknown;
  readonly responseSchema: unknown;
}

function schemaTransitionViolations(
  name: string,
  released: SchemaTransition,
  candidate: SchemaTransition,
): CompatibilityViolation[] {
  const violations: CompatibilityViolation[] = [];
  if (candidate.version.major !== released.version.major) {
    violations.push({
      method: name,
      rule: 'major-changed',
      detail: `${name}: major changed ${released.version.major} -> ${candidate.version.major}; majors never interoperate`,
    });
    return violations;
  }
  if (candidate.version.minor < released.version.minor) {
    violations.push({
      method: name,
      rule: 'minor-regressed',
      detail: `${name}: minor regressed ${released.version.minor} -> ${candidate.version.minor}`,
    });
  }
  const requestChanged = !canonicalEqual(released.requestSchema, candidate.requestSchema);
  const responseChanged = !canonicalEqual(released.responseSchema, candidate.responseSchema);
  // Any schema change must be released as a minor bump; an unchanged schema
  // may keep the same version.
  if ((requestChanged || responseChanged) && candidate.version.minor === released.version.minor) {
    const changed = [requestChanged ? 'request' : null, responseChanged ? 'response' : null]
      .filter((part) => part !== null)
      .join(' and ');
    violations.push({
      method: name,
      rule: 'minor-bump-required',
      detail: `${name}: ${changed} schema changed without a minor version bump (${released.version.major}.${released.version.minor})`,
    });
  }
  if (candidate.version.minor > released.version.minor || requestChanged || responseChanged) {
    if (!isSchemaBackwardCompatible(released.requestSchema, candidate.requestSchema)) {
      violations.push({
        method: name,
        rule: 'breaking-schema-change',
        detail: `${name}: request schema is not backward compatible within major ${candidate.version.major}`,
      });
    }
    if (!isResponseSchemaBackwardCompatible(released.responseSchema, candidate.responseSchema)) {
      violations.push({
        method: name,
        rule: 'breaking-schema-change',
        detail: `${name}: response schema is not backward compatible within major ${candidate.version.major}`,
      });
    }
  }
  return violations;
}

/** Validates one method's version/schema transition against its released definition. */
export function validateMethodTransition(
  released: MethodDefinition,
  candidate: MethodDefinition,
): CompatibilityViolation[] {
  return schemaTransitionViolations(
    released.name,
    {
      version: released.version,
      requestSchema: jsonSchemaOf(released.request),
      responseSchema: jsonSchemaOf(released.response),
    },
    {
      version: candidate.version,
      requestSchema: jsonSchemaOf(candidate.request),
      responseSchema: jsonSchemaOf(candidate.response),
    },
  );
}

/**
 * Validates a candidate method list against the released one:
 * - every released method must still exist (`method-removed`);
 * - shared methods are checked with [`validateMethodTransition`].
 */
export function validateManifestTransition(
  released: readonly MethodDefinition[],
  candidate: readonly MethodDefinition[],
): CompatibilityViolation[] {
  const violations: CompatibilityViolation[] = [];
  const candidateByName = new Map(candidate.map((m) => [m.name, m]));
  for (const releasedMethod of released) {
    const next = candidateByName.get(releasedMethod.name);
    if (next === undefined) {
      violations.push({
        method: releasedMethod.name,
        rule: 'method-removed',
        detail: `${releasedMethod.name}: released methods can never be removed`,
      });
      continue;
    }
    violations.push(...validateMethodTransition(releasedMethod, next));
  }
  return violations;
}

/**
 * One entry of the frozen released-contract baseline
 * (`packages/protocol-ts/released-contract.json`): a method as it was at the
 * last intentional contract release, with its full JSON Schemas.
 */
export interface FrozenContractMethod extends SchemaTransition {
  readonly name: string;
}

/**
 * Validates the candidate registry against the frozen released-contract
 * baseline. This is the release gate: every baseline method must still exist,
 * and each transition must be an additive minor. Regenerating candidate
 * outputs cannot silently bless same-version or breaking-minor edits; only
 * the explicit `release:contract` update action moves the baseline.
 */
export function validateFrozenContractTransition(
  released: readonly FrozenContractMethod[],
  candidate: readonly FrozenContractMethod[],
): CompatibilityViolation[] {
  const violations: CompatibilityViolation[] = [];
  const candidateByName = new Map(candidate.map((m) => [m.name, m]));
  for (const releasedMethod of released) {
    const next = candidateByName.get(releasedMethod.name);
    if (next === undefined) {
      violations.push({
        method: releasedMethod.name,
        rule: 'method-removed',
        detail: `${releasedMethod.name}: released methods can never be removed`,
      });
      continue;
    }
    violations.push(...schemaTransitionViolations(releasedMethod.name, releasedMethod, next));
  }
  return violations;
}

/**
 * One entry of the frozen released-contract baseline's `errorEnvelope`
 * section: the shared wire error shape as it was at the last intentional
 * contract release.
 */
export interface FrozenErrorEnvelope extends SchemaTransition {
  readonly name: string;
}

/**
 * The error-envelope counterpart of [`validateFrozenContractTransition`]:
 * the versioned `{protocol.error}` wire shape must survive as an additive
 * transition exactly like a method payload, so retryability or code changes
 * can never bypass the release gates.
 */
export function validateFrozenErrorEnvelopeTransition(
  released: FrozenErrorEnvelope,
  candidate: FrozenErrorEnvelope,
): CompatibilityViolation[] {
  return schemaTransitionViolations(released.name, released, candidate);
}
