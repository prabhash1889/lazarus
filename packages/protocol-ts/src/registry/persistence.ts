import { z } from 'zod';

import { canonicalJson, sha256Hex } from './fingerprint.ts';

/**
 * Persistence-record registry skeleton.
 *
 * Persisted records version independently from RPC methods: a record's
 * `{major, minor}` describes on-disk layout only and must never be conflated
 * with the wire contract (see docs/protocol/compatibility.md). This skeleton
 * establishes the namespace and snapshot mechanics; Phase 2 registers the
 * actual record types here.
 */
export const PERSISTENCE_REGISTRY_NAMESPACE = 'lazarus.persistence.v1';

export interface PersistenceRecordDefinition {
  /** Fully qualified record name, e.g. `task.record`. */
  readonly name: string;
  /** On-disk layout version. Independent of any method version. */
  readonly version: { readonly major: number; readonly minor: number };
  /** Zod schema of the persisted shape, when the record is registered. */
  readonly schema: z.ZodType;
}

/**
 * Registered persistence records. Empty until Phase 2 lands storage; the
 * registry API below is already exercised so conflation bugs cannot creep
 * in later.
 */
export const PERSISTENCE_RECORDS: readonly PersistenceRecordDefinition[] = [];

export interface PersistenceRecordSnapshot {
  readonly name: string;
  readonly version: { readonly major: number; readonly minor: number };
  readonly schemaFingerprint: string;
}

export interface PersistenceRegistrySnapshot {
  readonly namespace: typeof PERSISTENCE_REGISTRY_NAMESPACE;
  readonly records: readonly PersistenceRecordSnapshot[];
  readonly registryFingerprint: string;
}

/** Snapshots the persistence registry deterministically (same form as RPC manifest). */
export function snapshotPersistenceRegistry(
  records: readonly PersistenceRecordDefinition[] = PERSISTENCE_RECORDS,
): PersistenceRegistrySnapshot {
  const snapshots = records
    .map((record) => ({
      name: record.name,
      version: record.version,
      schemaFingerprint: sha256Hex(
        canonicalJson(z.toJSONSchema(record.schema, { io: 'input', reused: 'inline' })),
      ),
    }))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  return {
    namespace: PERSISTENCE_REGISTRY_NAMESPACE,
    records: snapshots,
    registryFingerprint: sha256Hex(canonicalJson(snapshots)),
  };
}
