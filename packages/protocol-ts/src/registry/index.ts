export {
  METHODS,
  snapshotMethod,
  snapshotManifest,
  methodByName,
  assertRegistryInvariants,
} from './methods.ts';
export {
  canonicalJson,
  jsonSchemaOf,
  sha256Hex,
  fingerprintSchema,
  isFingerprint,
} from './fingerprint.ts';
export {
  isSchemaBackwardCompatible,
  validateMethodTransition,
  validateManifestTransition,
  validateFrozenContractTransition,
} from './validation.ts';
export type { SchemaTransition, FrozenContractMethod } from './validation.ts';
export {
  BRIDGES,
  declareBridge,
  findBridge,
  bridgeFor,
  bridgedOlderMinors,
  isInteroperable,
  requiredBridgeCoverageViolations,
  resolveMethodSupport,
  adaptNewerToOlder,
} from './bridges.ts';
export type { MethodBridge, BridgeStep } from './bridges.ts';
export { RELEASED_FLOOR, releasedFloorGaps, releasedFloorSnapshot } from './released-floor.ts';
export {
  PERSISTENCE_REGISTRY_NAMESPACE,
  PERSISTENCE_RECORDS,
  snapshotPersistenceRegistry,
} from './persistence.ts';
export type {
  PersistenceRecordDefinition,
  PersistenceRecordSnapshot,
  PersistenceRegistrySnapshot,
} from './persistence.ts';
export type {
  MethodVersion,
  MethodKind,
  MethodSupport,
  MethodDefinition,
  MethodSnapshot,
  ManifestSnapshot,
  CompatibilityViolation,
} from './types.ts';

import { assertRegistryInvariants } from './methods.ts';

// Production bridges must be declared before the registry invariants run.
import './declared-bridges.ts';

// Fail fast at import time if the registry is internally inconsistent.
assertRegistryInvariants();
