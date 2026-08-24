import type { ManifestSnapshot, MethodDefinition, MethodSnapshot } from './types.ts';
import { fingerprintSchema, isFingerprint, sha256Hex } from './fingerprint.ts';
import { canonicalJson } from './fingerprint.ts';
import { BRIDGES } from './bridges.ts';

import {
  ListProcessesRequestSchema,
  ListProcessesResponseSchema,
  ProcessOutputRequestSchema,
  ProcessOutputResponseSchema,
  StartProcessRequestSchema,
  StartProcessResponseSchema,
  StopProcessRequestSchema,
  StopProcessResponseSchema,
} from './schemas/process.ts';
import {
  GetInfoRequestSchema,
  GetInfoResponseSchema,
  HealthRequestSchema,
  HealthResponseSchema,
  SubscribeEventsRequestSchema,
} from './schemas/system.ts';
import { EventFrameSchema } from './schemas/system.ts';
import { ListWorkspacesRequestSchema, ListWorkspacesResponseSchema } from './schemas/workspace.ts';
import { ListTasksRequestSchema, ListTasksResponseSchema } from './schemas/task.ts';

/**
 * The RPC surface. Every entry carries its own `{major, minor}`
 * version; there is no global protocol version. New methods are appended;
 * existing entries only move forward through additive minors (enforced by
 * `validation.validateManifestTransition`).
 */
export const METHODS: readonly MethodDefinition[] = [
  {
    name: 'system.health',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: HealthRequestSchema,
    response: HealthResponseSchema,
    optional: false,
  },
  {
    name: 'system.subscribeEvents',
    kind: 'serverStreaming',
    version: { major: 1, minor: 0 },
    request: SubscribeEventsRequestSchema,
    response: EventFrameSchema,
    optional: false,
  },
  {
    name: 'system.getInfo',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: GetInfoRequestSchema,
    response: GetInfoResponseSchema,
    optional: false,
  },
  {
    name: 'workspace.list',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: ListWorkspacesRequestSchema,
    response: ListWorkspacesResponseSchema,
    optional: false,
  },
  {
    name: 'task.list',
    kind: 'unary',
    version: { major: 1, minor: 2 },
    request: ListTasksRequestSchema,
    response: ListTasksResponseSchema,
    optional: false,
  },
  {
    name: 'process.start',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: StartProcessRequestSchema,
    response: StartProcessResponseSchema,
    optional: false,
  },
  {
    name: 'process.stop',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: StopProcessRequestSchema,
    response: StopProcessResponseSchema,
    optional: false,
  },
  {
    name: 'process.list',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: ListProcessesRequestSchema,
    response: ListProcessesResponseSchema,
    optional: false,
  },
  {
    name: 'process.output',
    kind: 'unary',
    version: { major: 1, minor: 0 },
    request: ProcessOutputRequestSchema,
    response: ProcessOutputResponseSchema,
    optional: false,
  },
];

/** Renders one method into its plain-data snapshot form. */
export function snapshotMethod(method: MethodDefinition): MethodSnapshot {
  return {
    name: method.name,
    kind: method.kind,
    version: method.version,
    optional: method.optional,
    ...(method.fallback === undefined ? {} : { fallback: method.fallback }),
    requestFingerprint: fingerprintSchema(method.request),
    responseFingerprint: fingerprintSchema(method.response),
  };
}

/** Snapshots a method list into the codegen interchange form. */
export function snapshotManifest(methods: readonly MethodDefinition[] = METHODS): ManifestSnapshot {
  const snapshots = methods
    .map(snapshotMethod)
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  return {
    methods: snapshots,
    manifestFingerprint: sha256Hex(canonicalJson(snapshots)),
  };
}

/** Looks up a method by fully qualified name. */
export function methodByName(name: string): MethodDefinition | undefined {
  return METHODS.find((method) => method.name === name);
}

/** Registry sanity: every method must produce well-formed fingerprints. */
export function assertRegistryInvariants(): void {
  const seen = new Set<string>();
  for (const method of METHODS) {
    if (seen.has(method.name)) {
      throw new Error(`duplicate method name: ${method.name}`);
    }
    seen.add(method.name);
    if (!isFingerprint(fingerprintSchema(method.request))) {
      throw new Error(`bad request fingerprint for ${method.name}`);
    }
    if (!isFingerprint(fingerprintSchema(method.response))) {
      throw new Error(`bad response fingerprint for ${method.name}`);
    }
    if (method.fallback !== undefined && !METHODS.some((m) => m.name === method.fallback)) {
      throw new Error(`fallback ${method.fallback} of ${method.name} is not a registered method`);
    }
  }
  for (const bridge of BRIDGES.values()) {
    const method = METHODS.find((m) => m.name === bridge.method);
    if (method === undefined) {
      throw new Error(`bridge targets unregistered method ${bridge.method}`);
    }
    if (
      bridge.newer.major !== method.version.major ||
      bridge.newer.minor !== method.version.minor
    ) {
      throw new Error(
        `bridge for ${bridge.method} targets ${bridge.newer.major}.${bridge.newer.minor}, ` +
          `but the registry serves ${method.version.major}.${method.version.minor}`,
      );
    }
  }
}
