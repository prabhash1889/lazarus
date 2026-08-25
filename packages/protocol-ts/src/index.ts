export { IDEMPOTENCY_KEY_HEADER } from './version.ts';

// The TypeScript/Zod contract registry: the single source of truth for the
// Lazarus Protocol. JSON Schema fingerprints and the generated Rust bindings
// (crates/protocol-rs/src/generated_registry.rs) are derived from it.
export {
  METHODS,
  snapshotMethod,
  snapshotManifest,
  methodByName,
  assertRegistryInvariants,
  canonicalJson,
  jsonSchemaOf,
  sha256Hex,
  fingerprintSchema,
  isFingerprint,
  isSchemaBackwardCompatible,
  validateMethodTransition,
  validateManifestTransition,
  declareBridge,
  findBridge,
  bridgeFor,
  isInteroperable,
  resolveMethodSupport,
  RELEASED_FLOOR,
  releasedFloorGaps,
  releasedFloorSnapshot,
  PERSISTENCE_REGISTRY_NAMESPACE,
  PERSISTENCE_RECORDS,
  snapshotPersistenceRegistry,
  ERROR_ENVELOPE,
  ERROR_ENVELOPE_NAME,
  ERROR_ENVELOPE_VERSION,
  errorEnvelopeFingerprint,
  isRetryableErrorCode,
  protocolError,
  RETRYABLE_ERROR_CODES,
  ERROR_CODE_OPTIONS,
} from './registry/index.ts';
export type {
  MethodVersion,
  MethodKind,
  MethodSupport,
  MethodDefinition,
  MethodSnapshot,
  ManifestSnapshot,
  CompatibilityViolation,
  MethodBridge,
  PersistenceRecordDefinition,
  PersistenceRecordSnapshot,
  PersistenceRegistrySnapshot,
  ProtocolErrorCode,
} from './registry/index.ts';
// Payload schemas clients decode responses with directly (the method
// registry above carries the same schemas; these named exports keep
// call sites free of deep package-internal imports).
export {
  EventFrameSchema,
  GetInfoRequestSchema,
  GetInfoResponseSchema,
  HealthRequestSchema,
  HealthResponseSchema,
} from './registry/schemas/system.ts';
export { ProtocolErrorSchema, ErrorCodeSchema } from './registry/schemas/common.ts';
