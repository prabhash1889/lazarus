#!/usr/bin/env node
// Generates the Rust contract bindings from the TypeScript/Zod protocol
// registry (@lazarus/protocol-ts), which is the single source of truth.
//
// Outputs:
//   crates/protocol-rs/src/generated_registry.rs   method manifest +
//                                                  fingerprints + generated
//                                                  wire types (the `wire`
//                                                  module: request/response
//                                                  payloads with decode
//                                                  validation, and the
//                                                  versioned error envelope)
//   crates/protocol-rs/tests/protocol_manifest.json golden snapshot for the
//                                                  cross-language equality test
//   crates/protocol-rs/tests/wire_fixtures.json    canonical sample payloads
//                                                  rendered and Zod-validated
//                                                  by the TypeScript registry;
//                                                  the Rust decoders must
//                                                  accept exactly these
//
// Release gate: before anything is written, the candidate registry is
// validated against the frozen released-contract baseline
// (packages/protocol-ts/released-contract.json). Same-version edits and
// breaking-minor edits fail generation - for method payloads and for the
// shared error envelope alike. The baseline is NEVER rewritten by normal
// generation; an intentional contract release moves it explicitly via:
//   pnpm --filter @lazarus/protocol-ts release:contract
//
// Run via: pnpm --filter @lazarus/protocol-ts gen:bindings
// Pass --check to verify the generated files are up to date without writing
// (exits non-zero on drift); used by CI.

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { format } from 'prettier';

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..');
const {
  METHODS,
  RELEASED_FLOOR,
  snapshotManifest,
  bridgedOlderMinors,
  jsonSchemaOf,
  canonicalJson,
  validateFrozenContractTransition,
  validateFrozenErrorEnvelopeTransition,
  requiredBridgeCoverageViolations,
  BRIDGES,
  ERROR_ENVELOPE,
  errorEnvelopeFingerprint,
  RETRYABLE_ERROR_CODES,
  protocolError,
  ERROR_CODE_OPTIONS,
} = await import(new URL('../packages/protocol-ts/src/registry/index.ts', import.meta.url));

const manifest = snapshotManifest(METHODS);

const baselinePath = join(repoRoot, 'packages', 'protocol-ts', 'released-contract.json');

async function readBaseline() {
  try {
    return JSON.parse(await readFile(baselinePath, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

function candidateContractMethods() {
  return METHODS.map((method) => ({
    name: method.name,
    version: method.version,
    requestSchema: JSON.parse(canonicalJson(jsonSchemaOf(method.request))),
    responseSchema: JSON.parse(canonicalJson(jsonSchemaOf(method.response))),
  }));
}

function candidateErrorEnvelope() {
  return {
    name: ERROR_ENVELOPE.name,
    version: { ...ERROR_ENVELOPE.version },
    requestSchema: {},
    responseSchema: JSON.parse(canonicalJson(jsonSchemaOf(ERROR_ENVELOPE.schema))),
  };
}

function rustMethodKind(kind) {
  if (kind === 'unary') return 'MethodKind::Unary';
  if (kind === 'serverStreaming') return 'MethodKind::ServerStreaming';
  throw new Error(`unknown method kind: ${kind}`);
}

const methodsSorted = [...manifest.methods].sort((a, b) =>
  a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
);
const floorSorted = [...RELEASED_FLOOR.keys()].sort();
const bridgesMinorsSorted = METHODS.map((method) => ({
  name: method.name,
  minors: [...bridgedOlderMinors(method)],
}))
  .filter((entry) => entry.minors.length > 0)
  .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
// Every declared bridge is serialized as data so the Host executes the
// identical declaration the TypeScript registry validated.
const bridgeBindingsSorted = [...BRIDGES.values()]
  .map((bridge) => ({
    name: bridge.method,
    olderMinor: bridge.older.minor,
    newerMinor: bridge.newer.minor,
    steps: bridge.steps,
  }))
  .sort((a, b) => (a.name === b.name ? a.olderMinor - b.olderMinor : a.name < b.name ? -1 : 1));

for (const method of methodsSorted) {
  if (!RELEASED_FLOOR.has(method.name)) {
    throw new Error(
      `${method.name} is registered but missing from the released floor; extend RELEASED_FLOOR before regenerating`,
    );
  }
}
for (const name of floorSorted) {
  if (!methodsSorted.some((m) => m.name === name)) {
    throw new Error(`released-floor method ${name} is not in the registry`);
  }
}

// ---------------------------------------------------------------------------
// Generated Rust wire types.
//
// Every method payload and the shared error envelope become real Rust types
// with serde derives plus a generated `validate()` that enforces exactly the
// constraints the JSON Schema carries (bounds, lengths). Serde's decoder
// enforces required fields and rejects wrong types while ignoring unknown
// additive fields, mirroring the TypeScript clients. Anything the walker
// does not understand fails generation loudly instead of emitting a lossy
// type: extend the generator together with the TypeScript registry.
// ---------------------------------------------------------------------------

// Annotation keywords never constrain acceptance and are ignored here.
const WIRE_ANNOTATION_KEYWORDS = new Set([
  '$schema',
  '$id',
  'title',
  'description',
  'default',
  'examples',
  'deprecated',
]);
// The structural keywords this generator understands.
const WIRE_HANDLED_KEYWORDS = new Set([
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
  'propertyNames',
  'additionalProperties',
]);

function assertSupported(schema, where) {
  if (typeof schema !== 'object' || schema === null) {
    throw new Error(`wire generator cannot render ${where}: not a schema object`);
  }
  for (const key of Object.keys(schema)) {
    if (!WIRE_ANNOTATION_KEYWORDS.has(key) && !WIRE_HANDLED_KEYWORDS.has(key)) {
      throw new Error(
        `wire generator cannot render ${where}: unsupported schema keyword "${key}"; extend the generator alongside the TypeScript registry`,
      );
    }
  }
}

/** `pageSize` -> `page_size`, `servedAtUnixMs` -> `served_at_unix_ms`. */
function rustFieldIdent(camel) {
  return camel.replace(/([A-Z])/g, '_$1').toLowerCase();
}

/** `system.subscribeEvents` -> `SystemSubscribeEvents`; `NOT_SERVING` -> `NotServing`. */
function rustPascal(name) {
  return name
    .split(/[^A-Za-z0-9_]+/)
    .filter(Boolean)
    .map((word) => {
      // Title-case per segment for snake words and ALL-CAPS values;
      // camelCase words keep their internal capitals after the lead.
      const titleCase = (part) => part[0].toUpperCase() + part.slice(1).toLowerCase();
      if (word.includes('_')) {
        return word.split('_').filter(Boolean).map(titleCase).join('');
      }
      if (!/[a-z]/.test(word)) return titleCase(word);
      return word[0].toUpperCase() + word.slice(1);
    })
    .join('');
}

function rustDecodeFn(methodName, role) {
  const snake = methodName
    .replaceAll('.', '_')
    .replace(/([A-Z])/g, '_$1')
    .toLowerCase();
  return `decode_${snake}_${role}`;
}

function unionBranchesWithTags(schema, where) {
  if (!Array.isArray(schema.oneOf)) return null;
  return schema.oneOf.map((branch, index) => {
    const tag = branch.properties?.type?.const;
    if (typeof tag !== 'string') {
      throw new Error(
        `wire generator cannot render ${where}: union branch ${index} lacks a const type tag`,
      );
    }
    return { tag, schema: branch };
  });
}

function isRecordNode(node) {
  return (
    node.type === 'object' &&
    node.additionalProperties !== undefined &&
    node.additionalProperties !== false &&
    Object.keys(node.properties ?? {}).length === 0
  );
}

/**
 * Constraint-check blocks for one property.
 *
 * `mode` selects how the value reaches the checks: `"self"` reads
 * `self.<field>` (struct impls), `"binding"` reads a match-arm binding named
 * `<field>` (union variant arms). Strings arrive as `&String`, integers as
 * `&u64`; optionals are unwrapped first. Callers indent every emitted line.
 */
function constraintLines(label, field, node, optional, mode) {
  const blocks = [];
  const recv = mode === 'self' ? `self.${field}` : field;
  // The borrow prelude is emitted lazily with the first check that needs it,
  // so unconstrained fields never produce an unused variable.
  let preludeEmitted = false;
  const ensurePrelude = () => {
    if (mode === 'self' && !optional && !preludeEmitted) {
      blocks.push(`let v = &${recv};`);
      preludeEmitted = true;
    }
  };
  const pushCheck = (condition, message) => {
    ensurePrelude();
    blocks.push(
      [
        `if ${condition} {`,
        `    return Err(${JSON.stringify(`${label}: ${message}`)}.to_string());`,
        `}`,
      ].join('\n'),
    );
  };
  // clippy::len_zero prefers is_empty over a length comparison against one.
  const lenCompare = (target, op, bound) =>
    op === '<' && bound === 1 ? `${target}.is_empty()` : `${target}.len() ${op} ${bound}`;
  const wrapOptional = (inner) => {
    const recvExpr = mode === 'self' ? `self.${field}.as_ref()` : `${field}.as_ref()`;
    return `${recvExpr}.is_some_and(|v| ${inner})`;
  };
  if (node.type === 'string') {
    const direct = mode === 'self' && !optional ? 'v' : field;
    if (node.minLength !== undefined) {
      pushCheck(
        optional
          ? wrapOptional(lenCompare('v', '<', node.minLength))
          : lenCompare(direct, '<', node.minLength),
        `must be at least ${node.minLength} characters`,
      );
    }
    if (node.maxLength !== undefined) {
      pushCheck(
        optional
          ? wrapOptional(lenCompare('v', '>', node.maxLength))
          : lenCompare(direct, '>', node.maxLength),
        `must be at most ${node.maxLength} characters`,
      );
    }
  }
  if (node.type === 'integer') {
    // u64 already implies minimum >= 0; only non-trivial bounds render.
    if (typeof node.minimum === 'number' && node.minimum > 0) {
      pushCheck(
        optional
          ? `${recv}.as_ref().is_some_and(|v| *v < ${node.minimum})`
          : mode === 'self'
            ? `*v < ${node.minimum}`
            : `*${field} < ${node.minimum}`,
        `must be at least ${node.minimum}`,
      );
    }
    if (typeof node.maximum === 'number') {
      pushCheck(
        optional
          ? `${recv}.as_ref().is_some_and(|v| *v > ${node.maximum})`
          : mode === 'self'
            ? `*v > ${node.maximum}`
            : `*${field} > ${node.maximum}`,
        `must be at most ${node.maximum}`,
      );
    }
  }
  return blocks;
}

/** Collects the Rust definitions for one payload schema. */
function collectPayloadTypes(rootIdent, schema) {
  const defs = [];
  let usesHashMap = false;

  function convert(ident, node, where) {
    assertSupported(node, where);
    const branches = unionBranchesWithTags(node, where);
    if (branches !== null) {
      emitTaggedUnion(ident, branches, where);
      return ident;
    }
    if (node.enum !== undefined) {
      emitStringEnum(ident, node, where);
      return ident;
    }
    switch (node.type) {
      case 'string':
        return 'String';
      case 'integer':
        return 'u64';
      case 'boolean':
        return 'bool';
      case 'array': {
        assertSupported(node.items, `${where}[]`);
        const itemType = convert(`${ident}Item`, node.items, `${where}[]`);
        return `Vec<${itemType}>`;
      }
      case 'object': {
        if (isRecordNode(node)) {
          const valueNode = node.additionalProperties;
          assertSupported(valueNode, `${where}<*>`);
          if (valueNode.type === 'boolean' && unionBranchesWithTags(valueNode, where) === null) {
            usesHashMap = true;
            return 'HashMap<String, bool>';
          }
          throw new Error(
            `wire generator cannot render ${where}: only record<string, boolean> maps are supported`,
          );
        }
        emitStruct(ident, node, where);
        return ident;
      }
      default:
        throw new Error(`wire generator cannot render ${where}: unsupported type ${node.type}`);
    }
  }

  function emitStruct(ident, node, where) {
    const required = new Set(node.required ?? []);
    const properties = node.properties ?? {};
    const fieldLines = [];
    let checks = '';
    for (const [fieldName, propertyNode] of Object.entries(properties)) {
      assertSupported(propertyNode, `${where}.${fieldName}`);
      if (
        unionBranchesWithTags(propertyNode, where) === null &&
        propertyNode.enum === undefined &&
        propertyNode.const !== undefined
      ) {
        throw new Error(
          `wire generator cannot render ${where}.${fieldName}: const outside a union tag`,
        );
      }
      const optional = !required.has(fieldName);
      const rustField = rustFieldIdent(fieldName);
      const rustType = convert(
        `${ident}${rustPascal(fieldName)}`,
        propertyNode,
        `${where}.${fieldName}`,
      );
      const attrs = [];
      if (rustField !== fieldName) attrs.push(`rename = ${JSON.stringify(fieldName)}`);
      if (optional) attrs.push(`skip_serializing_if = "Option::is_none"`);
      const renderedType = optional ? `Option<${rustType}>` : rustType;
      fieldLines.push(
        ...(attrs.length > 0 ? [`#[serde(${attrs.join(', ')})]`] : []),
        `pub ${rustField}: ${renderedType},`,
      );
      for (const block of constraintLines(fieldName, rustField, propertyNode, optional, 'self')) {
        checks += `${indentBlock(block, 8)}\n`;
      }
    }
    const structDecl =
      fieldLines.length === 0
        ? `#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]\npub struct ${ident} {}`
        : [
            `#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`,
            `pub struct ${ident} {`,
            ...fieldLines.map((line) => `    ${line}`),
            `}`,
          ].join('\n');
    defs.push({
      ident,
      code: [
        structDecl,
        ``,
        `impl ${ident} {`,
        `    /// Enforces exactly the constraints carried by the contract schema.`,
        `    pub fn validate(&self) -> Result<(), String> {`,
        ...(checks.length > 0
          ? checks.trimEnd().split('\n')
          : [`        // No constraints beyond serde's decoding.`]),
        `        Ok(())`,
        `    }`,
        `}`,
      ].join('\n'),
    });
  }

  function emitStringEnum(ident, node, where) {
    if (node.type !== 'string') {
      throw new Error(`wire generator cannot render ${where}: unsupported enum of ${node.type}`);
    }
    const variantLines = [];
    const armLines = [];
    for (const value of node.enum) {
      const variant = rustPascal(String(value));
      variantLines.push(`#[serde(rename = ${JSON.stringify(value)})]`, `${variant},`);
      armLines.push(`Self::${variant} => ${JSON.stringify(value)},`);
    }
    defs.push({
      ident,
      code: [
        `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`,
        `pub enum ${ident} {`,
        ...variantLines.map((line) => `    ${line}`),
        `}`,
        ``,
        `impl ${ident} {`,
        `    pub fn as_str(self) -> &'static str {`,
        `        match self {`,
        ...armLines.map((line) => `            ${line}`),
        `        }`,
        `    }`,
        `}`,
      ].join('\n'),
    });
  }

  function emitTaggedUnion(ident, branches, where) {
    const variantsRust = [];
    const armRust = [];
    for (const branch of branches) {
      const variant = rustPascal(branch.tag);
      const node = { ...branch.schema };
      const properties = { ...(node.properties ?? {}) };
      const required = new Set(node.required ?? []);
      delete properties.type;
      required.delete('type');
      const fieldLines = [];
      let checks = '';
      const bindingNames = [];
      for (const [fieldName, propertyNode] of Object.entries(properties)) {
        assertSupported(propertyNode, `${where}.${branch.tag}.${fieldName}`);
        const optional = !required.has(fieldName);
        const rustField = rustFieldIdent(fieldName);
        bindingNames.push(rustField);
        const rustType = convert(
          `${ident}${variant}${rustPascal(fieldName)}`,
          propertyNode,
          `${where}.${branch.tag}.${fieldName}`,
        );
        const renderedType = optional ? `Option<${rustType}>` : rustType;
        fieldLines.push(
          ...(optional ? [`#[serde(skip_serializing_if = "Option::is_none")]`] : []),
          `${rustField}: ${renderedType},`,
        );
        for (const block of constraintLines(
          fieldName,
          rustField,
          propertyNode,
          optional,
          'binding',
        )) {
          checks += `${indentBlock(block, 16)}\n`;
        }
      }
      variantsRust.push(
        indentBlock(
          [`${variant} {`, ...fieldLines.map((line) => `    ${line}`), `},`].join('\n'),
          4,
        ),
      );
      armRust.push(
        checks.length > 0
          ? [
              `            Self::${variant} { ${bindingNames.join(', ')} } => {`,
              checks.trimEnd(),
              `                Ok(())`,
              `            }`,
            ].join('\n')
          : `            Self::${variant} { .. } => Ok(()),`,
      );
    }
    defs.push({
      ident,
      code: [
        `#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`,
        `#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]`,
        `pub enum ${ident} {`,
        ...variantsRust,
        `}`,
        ``,
        `impl ${ident} {`,
        `    /// Enforces exactly the constraints carried by the contract schema.`,
        `    pub fn validate(&self) -> Result<(), String> {`,
        `        match self {`,
        ...armRust,
        `        }`,
        `    }`,
        `}`,
      ].join('\n'),
    });
  }

  const rootType = convert(rootIdent, schema, rootIdent);
  return { rootType, defs, usesHashMap };
}

function indentBlock(block, spaces) {
  const pad = ' '.repeat(spaces);
  return block
    .split('\n')
    .map((line) => pad + line)
    .join('\n');
}

function indent(text, spaces) {
  return text
    .split('\n')
    .map((line) => (line.length > 0 ? ' '.repeat(spaces) + line : line))
    .join('\n');
}

/** Emits the whole `wire` module: payload types, decoders, error envelope. */
function rustWireModule() {
  const defs = [];
  const decoders = [];
  let usesHashMap = false;

  for (const method of methodsSorted) {
    const definition = METHODS.find((entry) => entry.name === method.name);
    const pascal = rustPascal(method.name);
    for (const role of ['request', 'response']) {
      const zod = role === 'request' ? definition.request : definition.response;
      const schema = JSON.parse(canonicalJson(jsonSchemaOf(zod)));
      const collected = collectPayloadTypes(
        `${pascal}${role === 'request' ? 'Request' : 'Response'}`,
        schema,
      );
      if (collected.usesHashMap) usesHashMap = true;
      defs.push(...collected.defs);
      const label = `${method.name} ${role}`;
      decoders.push(
        [
          `/// Decodes and validates a \`${method.name}\` ${role} payload produced by any peer.`,
          `pub fn ${rustDecodeFn(method.name, role)}(`,
          `    value: &serde_json::Value,`,
          `) -> Result<${collected.rootType}, WireDecodeError> {`,
          `    let decoded: ${collected.rootType} = serde_json::from_value(value.clone())`,
          `        .map_err(|error| WireDecodeError::new(${JSON.stringify(label)}, error.to_string()))?;`,
          `    decoded`,
          `        .validate()`,
          `        .map_err(|reason| WireDecodeError::new(${JSON.stringify(label)}, reason))?;`,
          `    Ok(decoded)`,
          `}`,
        ].join('\n'),
      );
    }
  }

  // Identical definitions render once (shared shapes stay stable).
  const seenIdents = new Set();
  const uniqueDefs = defs.filter((def) => {
    if (seenIdents.has(def.ident)) return false;
    seenIdents.add(def.ident);
    return true;
  });

  const retryableVariants = [...RETRYABLE_ERROR_CODES]
    .map((code) => `Self::${rustPascal(code)}`)
    .join(' | ');

  const errorCodeVariants = ERROR_CODE_OPTIONS.map(
    (code) => `#[serde(rename = ${JSON.stringify(code)})]\n${rustPascal(code)},`,
  ).join('\n');
  const errorCodeArms = ERROR_CODE_OPTIONS.map(
    (code) => `Self::${rustPascal(code)} => ${JSON.stringify(code)},`,
  ).join('\n');

  return `/// Generated wire bindings for every protocol method payload and the
/// shared error envelope, derived from the same TypeScript/Zod schemas as
/// the manifest above. Decoders enforce required fields and reject wrong
/// types (via serde) plus every schema constraint (via \`validate()\`),
/// while unknown additive fields pass through untouched - exactly like the
/// TypeScript clients. Canonical sample payloads live at
/// crates/protocol-rs/tests/wire_fixtures.json. Do not edit by hand.
pub mod wire {${
    usesHashMap
      ? `
    use std::collections::HashMap;
`
      : ''
  }
    /// Why a wire payload failed to decode or violated its contract.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WireDecodeError {
        /// Which payload failed, e.g. \`"task.list response"\`.
        pub payload: &'static str,
        pub reason: String,
    }

    impl WireDecodeError {
        pub(crate) fn new(payload: &'static str, reason: impl Into<String>) -> Self {
            Self {
                payload,
                reason: reason.into(),
            }
        }
    }

    impl std::fmt::Display for WireDecodeError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} payload is invalid: {}", self.payload, self.reason)
        }
    }

    impl std::error::Error for WireDecodeError {}

    /// Major version of the versioned error-envelope contract
    /// (\`protocol.error\`), frozen in the released-contract baseline like
    /// every method payload.
    pub const ERROR_ENVELOPE_MAJOR: u32 = ${ERROR_ENVELOPE_VERSION_MAJOR};
    /// Minor version of the versioned error-envelope contract.
    pub const ERROR_ENVELOPE_MINOR: u32 = ${ERROR_ENVELOPE_VERSION_MINOR};
    /// SHA-256 (hex) of the canonical JSON Schema of the error envelope.
    pub const ERROR_ENVELOPE_FINGERPRINT: &str = "${errorEnvelopeFingerprint()}";

    /// The typed wire error envelope (\`protocol.error\`).
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProtocolError {
        pub code: ProtocolErrorCode,
        pub message: String,
        pub retryable: bool,
    }

    /// Every canonical error code, generated from the TypeScript schema.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    pub enum ProtocolErrorCode {
${indent(errorCodeVariants, 8)}
    }

    impl ProtocolErrorCode {
        pub fn as_str(self) -> &'static str {
            match self {
${indent(errorCodeArms, 16)}
            }
        }

        /// True when the canonical classification marks the code retryable:
        /// re-issuing the call (idempotency-keyed where it writes) may
        /// succeed. Every other code is terminal.
        pub fn is_retryable(self) -> bool {
            matches!(self, ${retryableVariants})
        }
    }

    impl ProtocolError {
        /// Builds a conforming envelope: \`retryable\` always comes from the
        /// canonical classification so call sites cannot mislabel an error.
        pub fn new(code: ProtocolErrorCode, message: impl Into<String>) -> Self {
            Self {
                retryable: code.is_retryable(),
                code,
                message: message.into(),
            }
        }
    }

    /// Decodes and validates an error envelope received from a peer.
    pub fn decode_protocol_error(
        value: &serde_json::Value,
    ) -> Result<ProtocolError, WireDecodeError> {
        let decoded: ProtocolError = serde_json::from_value(value.clone())
            .map_err(|error| WireDecodeError::new("protocol.error", error.to_string()))?;
        Ok(decoded)
    }

${uniqueDefs.map((def) => indent(def.code, 4)).join('\n\n')}
${decoders.map((decoder) => indent(decoder, 4)).join('\n\n')}
}
`;
}

const ERROR_ENVELOPE_VERSION_MAJOR = candidateErrorEnvelope().version.major;
const ERROR_ENVELOPE_VERSION_MINOR = candidateErrorEnvelope().version.minor;

// Canonical sample payloads: rendered from the released schemas here and
// Zod-verified before they are written, so the Rust decoders are proven
// against instances the TypeScript registry itself accepts.
function sampleFromSchema(schema, where) {
  assertSupported(schema, where);
  const branches = unionBranchesWithTags(schema, where);
  if (branches !== null) return sampleFromSchema(branches[0].schema, `${where}[0]`);
  if (schema.const !== undefined) return schema.const;
  if (schema.enum !== undefined) return schema.enum[0];
  switch (schema.type) {
    case 'string':
      return 'lazarus';
    case 'boolean':
      return true;
    case 'integer': {
      let value = typeof schema.minimum === 'number' ? schema.minimum : 0;
      if (typeof schema.maximum === 'number') value = Math.min(value, schema.maximum);
      return value;
    }
    case 'array':
      return [];
    case 'object': {
      if (isRecordNode(schema)) {
        return { lazarus: sampleFromSchema(schema.additionalProperties, `${where}<*>`) };
      }
      const sample = {};
      for (const key of schema.required ?? []) {
        sample[key] = sampleFromSchema(schema.properties[key], `${where}.${key}`);
      }
      return sample;
    }
    default:
      throw new Error(`fixture sampler cannot render ${where}: type ${schema.type}`);
  }
}

async function buildWireFixtures() {
  const methods = {};
  for (const method of [...METHODS].sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const requestSample = sampleFromSchema(jsonSchemaOf(method.request), `${method.name} request`);
    const responseSample = sampleFromSchema(
      jsonSchemaOf(method.response),
      `${method.name} response`,
    );
    // The TypeScript registry must accept exactly what we ship as canonical.
    void method.request.parse(requestSample);
    void method.response.parse(responseSample);
    methods[method.name] = { request: requestSample, response: responseSample };
  }
  const envelopeSample = protocolError('UNAVAILABLE', 'lazarus');
  void ERROR_ENVELOPE.schema.parse(envelopeSample);
  return {
    $comment:
      'Canonical sample payloads rendered from the TypeScript/Zod registry by ' +
      'scripts/generate-protocol-bindings.mjs and validated with the registry ' +
      'schemas before being written. The generated Rust decoders must accept ' +
      'these exact instances. Regenerate with: pnpm gen:protocol',
    errorEnvelope: envelopeSample,
    methods,
  };
}

const bridgesTableRust =
  bridgesMinorsSorted.length === 0
    ? '[]'
    : `[\n${bridgesMinorsSorted
        .map((entry) => `    (${JSON.stringify(entry.name)}, &[${entry.minors.join(', ')}]),`)
        .join('\n')}\n]`;
const bridgeBindingsRust =
  bridgeBindingsSorted.length === 0
    ? '[]'
    : `[\n${bridgeBindingsSorted
        .map(
          (bridge) => `    MethodBridgeBinding {
        name: ${JSON.stringify(bridge.name)},
        older_minor: ${bridge.olderMinor},
        steps: &[${
          bridge.steps.length === 0
            ? ''
            : `\n${bridge.steps.map(rustBridgeStep).join(',\n')},\n        `
        }],
    },`,
        )
        .join('\n')}\n]`;

function rustBridgeStep(step) {
  if (step.op === 'omitResponseFields') {
    return `            BridgeStep::OmitResponseFields(&[${step.fields
      .map((field) => JSON.stringify(field))
      .join(', ')}])`;
  }
  throw new Error(
    `unknown bridge step op: ${step.op}; extend the generator alongside the TypeScript executor`,
  );
}

const wireModuleRust = rustWireModule();

const rustOutRaw = `// @generated by scripts/generate-protocol-bindings.mjs from
// packages/protocol-ts/src/registry (the TypeScript/Zod source of truth).
// DO NOT EDIT BY HAND. Regenerate with:
//   pnpm --filter @lazarus/protocol-ts gen:bindings
//
// The manifest fingerprint equals the SHA-256 of the canonical JSON of the
// sorted method snapshots computed by the TypeScript registry
// (\`snapshotManifest()\`); the golden copy used for verification lives at
// crates/protocol-rs/tests/protocol_manifest.json. Canonical sample payloads
// for the generated wire decoders live at
// crates/protocol-rs/tests/wire_fixtures.json.

/// Transport shape of a protocol method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MethodKind {
    Unary,
    ServerStreaming,
}

impl MethodKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MethodKind::Unary => "unary",
            MethodKind::ServerStreaming => "serverStreaming",
        }
    }
}

/// One generated binding: the per-method contract anchor on the Rust side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodBinding {
    pub name: &'static str,
    pub kind: MethodKind,
    pub major: u32,
    pub minor: u32,
    pub optional: bool,
    pub fallback: Option<&'static str>,
    /// SHA-256 (hex) of the canonical JSON Schema of the request.
    pub request_fingerprint: &'static str,
    /// SHA-256 (hex) of the canonical JSON Schema of the response.
    pub response_fingerprint: &'static str,
}

/// Manifest fingerprint over all method bindings (see header).
pub const MANIFEST_FINGERPRINT: &str =
    "${manifest.manifestFingerprint}";

/// Generated method bindings, sorted by name.
pub const METHOD_BINDINGS: &[MethodBinding] = &[
${methodsSorted
  .map(
    (m) => `    MethodBinding {
        name: ${JSON.stringify(m.name)},
        kind: ${rustMethodKind(m.kind)},
        major: ${m.version.major},
        minor: ${m.version.minor},
        optional: ${m.optional},
        fallback: ${m.fallback === undefined ? 'None' : `Some(${JSON.stringify(m.fallback)})`},
        request_fingerprint: ${JSON.stringify(m.requestFingerprint)},
        response_fingerprint: ${JSON.stringify(m.responseFingerprint)},
    },`,
  )
  .join('\n')}
];

/// Frozen released-floor method names every supported Host keeps serving.
pub const RELEASED_FLOOR: &[&str] = &[
${floorSorted.map((name) => `    ${JSON.stringify(name)},`).join('\n')}
];

/// Older peer minors each method interoperates with through a declared
/// TypeScript bridge, sorted by method then minor. Derived from the bridge
/// registry in the source-of-truth package; a peer advertising one of these
/// minors negotiates at that minor instead of being rejected.
#[rustfmt::skip]
pub const BRIDGED_PEER_MINORS: &[(&str, &[u32])] = &${bridgesTableRust};

/// One declarative bridge step. Plain data only: the Host executes these
/// steps over response payloads, mirroring the TypeScript executor exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeStep {
    /// Removes named top-level fields added after the older peer minor.
    OmitResponseFields(&'static [&'static str]),
}

/// One generated bridge binding: the executable adapter keeping an older
/// peer minor interoperable with its method's current minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodBridgeBinding {
    pub name: &'static str,
    pub older_minor: u32,
    pub steps: &'static [BridgeStep],
}

/// Declared bridges, sorted by name then older minor.
#[rustfmt::skip]
pub const METHOD_BRIDGES: &[MethodBridgeBinding] = &${bridgeBindingsRust};

/// Declared-bridge peer minors for a method; empty when none are declared.
pub fn bridged_peer_minors(name: &str) -> &'static [u32] {
    BRIDGED_PEER_MINORS
        .iter()
        .find(|(method, _)| *method == name)
        .map(|(_, minors)| *minors)
        .unwrap_or(&[])
}

/// The declared bridge steps adapting \`name\` responses down to an older
/// peer minor; empty when no bridge is declared for that pair.
pub fn bridge_steps(name: &str, older_minor: u32) -> &'static [BridgeStep] {
    METHOD_BRIDGES
        .iter()
        .find(|bridge| bridge.name == name && bridge.older_minor == older_minor)
        .map(|bridge| bridge.steps)
        .unwrap_or(&[])
}

/// Looks up a generated binding by fully qualified method name.
pub fn binding_by_name(name: &str) -> Option<&'static MethodBinding> {
    METHOD_BINDINGS.iter().find(|binding| binding.name == name)
}

${wireModuleRust}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_sorted_and_unique() {
        let names: Vec<&str> = METHOD_BINDINGS.iter().map(|b| b.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "bindings must be sorted by unique name");
    }

    #[test]
    fn released_floor_is_covered_by_bindings() {
        for name in RELEASED_FLOOR {
            assert!(
                binding_by_name(name).is_some(),
                "released-floor method {name} has no binding"
            );
        }
        assert_eq!(RELEASED_FLOOR.len(), METHOD_BINDINGS.len());
    }

    #[test]
    fn fingerprints_are_sha256_hex() {
        assert_eq!(MANIFEST_FINGERPRINT.len(), 64);
        assert!(MANIFEST_FINGERPRINT.chars().all(|c| c.is_ascii_hexdigit()));
        for binding in METHOD_BINDINGS {
            for fingerprint in [binding.request_fingerprint, binding.response_fingerprint] {
                assert_eq!(
                    fingerprint.len(),
                    64,
                    "{:?} fingerprint length",
                    binding.name
                );
                assert!(
                    fingerprint.chars().all(|c| c.is_ascii_hexdigit()),
                    "{:?} fingerprint charset",
                    binding.name
                );
            }
            if binding.fallback.is_some() {
                assert!(binding.optional, "only optional methods declare fallbacks");
            }
        }
        assert_eq!(wire::ERROR_ENVELOPE_FINGERPRINT.len(), 64);
    }

    #[test]
    fn lookup_roundtrips() {
        let health = binding_by_name("system.health").expect("health binding");
        assert_eq!(health.kind, MethodKind::Unary);
        assert_eq!(health.major, 1);
        assert_eq!(health.minor, 0);
        let subscribe = binding_by_name("system.subscribeEvents").expect("subscribe binding");
        assert_eq!(subscribe.kind, MethodKind::ServerStreaming);
        assert!(binding_by_name("no.such_method").is_none());
    }

    #[test]
    fn bridged_minors_reference_bindings_and_stay_sorted() {
        let names: Vec<&str> = BRIDGED_PEER_MINORS.iter().map(|(n, _)| *n).collect();
        let mut sorted_names = names.clone();
        sorted_names.sort_unstable();
        sorted_names.dedup();
        assert_eq!(
            names, sorted_names,
            "bridge entries must be sorted by unique method"
        );
        for (name, minors) in BRIDGED_PEER_MINORS {
            assert!(
                binding_by_name(name).is_some(),
                "bridge for unknown binding {name}"
            );
            assert!(!minors.is_empty(), "{name} declares an empty bridge table");
            let mut sorted = minors.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                *minors,
                sorted.as_slice(),
                "{name} bridge minors sorted/unique"
            );
        }
        for binding in METHOD_BINDINGS {
            let looked_up = bridged_peer_minors(binding.name);
            let expected = BRIDGED_PEER_MINORS
                .iter()
                .find(|(n, _)| *n == binding.name)
                .map(|(_, m)| *m)
                .unwrap_or(&[]);
            assert_eq!(looked_up, expected, "bridged_peer_minors({})", binding.name);
        }
    }

    #[test]
    fn bridge_steps_match_bridged_peer_minors() {
        for bridge in METHOD_BRIDGES {
            assert!(
                binding_by_name(bridge.name).is_some(),
                "bridge for unknown binding {}",
                bridge.name
            );
            assert!(
                bridged_peer_minors(bridge.name).contains(&bridge.older_minor),
                "{} declares steps for unbridged minor {}",
                bridge.name,
                bridge.older_minor
            );
        }
        for (name, minors) in BRIDGED_PEER_MINORS {
            for minor in *minors {
                assert!(
                    METHOD_BRIDGES
                        .iter()
                        .any(|bridge| bridge.name == *name && bridge.older_minor == *minor),
                    "bridged minor {minor} of {name} has no executable steps"
                );
            }
        }
    }

    #[test]
    fn error_envelope_labels_retryability_from_the_canonical_classification() {
        assert!(wire::ProtocolErrorCode::Unavailable.is_retryable());
        assert!(wire::ProtocolErrorCode::DeadlineExceeded.is_retryable());
        for terminal in [
            wire::ProtocolErrorCode::Cancelled,
            wire::ProtocolErrorCode::Unknown,
            wire::ProtocolErrorCode::InvalidArgument,
            wire::ProtocolErrorCode::NotFound,
            wire::ProtocolErrorCode::AlreadyExists,
            wire::ProtocolErrorCode::PermissionDenied,
            wire::ProtocolErrorCode::Unauthenticated,
            wire::ProtocolErrorCode::FailedPrecondition,
            wire::ProtocolErrorCode::IncompatibleMethodManifest,
            wire::ProtocolErrorCode::Internal,
        ] {
            assert!(!terminal.is_retryable(), "{terminal:?} stays terminal");
        }
        let constructed =
            wire::ProtocolError::new(wire::ProtocolErrorCode::Unavailable, "host restarting");
        assert!(constructed.retryable);
        let decoded = wire::decode_protocol_error(&serde_json::to_value(&constructed).unwrap())
            .expect("roundtrip");
        assert_eq!(decoded, constructed);
        assert!(
            wire::decode_protocol_error(&serde_json::json!({"code": "INTERNAL"})).is_err(),
            "the retryability label is mandatory on the wire"
        );
    }

    #[test]
    fn decoders_reject_contract_violating_payloads() {
        // pageSize below the contracted floor fails validation...
        let bad_page = wire::decode_task_list_request(&serde_json::json!({"pageSize": 0}));
        assert!(bad_page.is_err(), "pageSize 0 violates the contract");
        // ...and so does a wrong-typed field.
        let bad_type =
            wire::decode_task_list_response(&serde_json::json!({"tasks": [{"id": 7, "title": "x", "status": "PENDING"}]}));
        assert!(bad_type.is_err(), "wrong types must fail decoding");
    }
}
`;

// The generated Rust must be byte-stable and rustfmt-clean: formatting
// through the project toolchain here guarantees both, so `cargo fmt --check`
// can never drift from regeneration. Requires the repo's Rust toolchain,
// which every generation context (contributors and CI) already has.
function formatRust(source) {
  const result = spawnSync('rustfmt', ['--edition', '2024', '--emit', 'stdout'], {
    input: source,
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.error !== undefined || result.status !== 0) {
    throw new Error(
      `rustfmt failed (${result.status ?? result.error}): ${result.stderr?.trim() ?? 'unknown error'}`,
    );
  }
  return result.stdout;
}

const rustOut = formatRust(rustOutRaw);

const jsonGolden = await format(
  JSON.stringify({
    manifestFingerprint: manifest.manifestFingerprint,
    methods: methodsSorted.map((m) => ({
      name: m.name,
      kind: m.kind,
      major: m.version.major,
      minor: m.version.minor,
      optional: m.optional,
      fallback: m.fallback ?? null,
      requestFingerprint: m.requestFingerprint,
      responseFingerprint: m.responseFingerprint,
    })),
    releasedFloor: floorSorted,
    bridges: bridgeBindingsSorted.map((bridge) => ({
      name: bridge.name,
      olderMinor: bridge.olderMinor,
      newerMinor: bridge.newerMinor,
      steps: bridge.steps,
    })),
    errorEnvelope: {
      name: candidateErrorEnvelope().name,
      major: ERROR_ENVELOPE_VERSION_MAJOR,
      minor: ERROR_ENVELOPE_VERSION_MINOR,
      fingerprint: errorEnvelopeFingerprint(),
    },
  }),
  { parser: 'json' },
);

const jsonFixtures = await format(JSON.stringify(await buildWireFixtures()), { parser: 'json' });

const check = process.argv.includes('--check');
const updateReleasedContract = process.argv.includes('--update-released-contract');

function validateCandidateAgainst(baseline) {
  const violations = [
    ...validateFrozenContractTransition(baseline.methods, candidateContractMethods()),
    ...requiredBridgeCoverageViolations(baseline.methods, METHODS),
  ];
  // Baselines written before the error envelope joined the frozen contract
  // are migrated by the explicit release action below; afterwards the
  // envelope is gated exactly like a method payload.
  if (baseline.errorEnvelope !== undefined) {
    violations.push(
      ...validateFrozenErrorEnvelopeTransition(baseline.errorEnvelope, candidateErrorEnvelope()),
    );
  }
  return violations;
}

function failOnViolations(violations) {
  if (violations.length === 0) return;
  console.error('ERROR: candidate registry breaks the frozen released contract:');
  for (const violation of violations) {
    console.error(`  [${violation.rule}] ${violation.detail}`);
  }
  console.error('Fix the regression or declare a bridge for the released minor.');
  process.exit(1);
}

// Explicit baseline update action: freeze the current registry as the new
// released contract. An existing baseline is validated before it can move,
// so this command cannot bless a regression that normal generation rejects.
if (updateReleasedContract) {
  const existingBaseline = await readBaseline();
  if (existingBaseline !== null) {
    failOnViolations(validateCandidateAgainst(existingBaseline));
  }
  const baseline = {
    $comment:
      'Frozen released-contract baseline: the method contracts and the shared ' +
      'error envelope as of the last intentional release. Generation and CI ' +
      'validate the candidate registry against this file; it is never rewritten ' +
      'by normal generation. Move it explicitly with: pnpm --filter @lazarus/protocol-ts release:contract',
    manifestFingerprint: manifest.manifestFingerprint,
    methods: METHODS.map((m) => ({
      name: m.name,
      version: { major: m.version.major, minor: m.version.minor },
      requestSchema: JSON.parse(canonicalJson(jsonSchemaOf(m.request))),
      responseSchema: JSON.parse(canonicalJson(jsonSchemaOf(m.response))),
    })),
    errorEnvelope: candidateErrorEnvelope(),
  };
  await mkdir(dirname(baselinePath), { recursive: true });
  await writeFile(
    baselinePath,
    await format(JSON.stringify(baseline, null, 2), { parser: 'json' }),
    'utf8',
  );
  console.log(`updated released-contract baseline: ${baselinePath}`);
  console.log(`released manifest fingerprint: ${manifest.manifestFingerprint}`);
} else {
  // Release gate: the candidate registry must be an additive-minor forward
  // transition from the frozen released contract, every required method
  // whose minor advanced must keep the released minor interoperable through
  // a declared bridge, and the error envelope must survive additively -
  // in both generate and --check modes.
  const baseline = await readBaseline();
  if (baseline === null) {
    console.error(
      `ERROR: released-contract baseline is missing: ${baselinePath}; create it with pnpm --filter @lazarus/protocol-ts release:contract`,
    );
    process.exit(1);
  }
  failOnViolations(validateCandidateAgainst(baseline));
  console.log(
    `candidate registry is a valid additive-minor transition from the released contract (${baseline.methods.length} methods)`,
  );
}

const rustPath = join(repoRoot, 'crates', 'protocol-rs', 'src', 'generated_registry.rs');
const jsonPath = join(repoRoot, 'crates', 'protocol-rs', 'tests', 'protocol_manifest.json');
const fixturesPath = join(repoRoot, 'crates', 'protocol-rs', 'tests', 'wire_fixtures.json');

async function readExisting(path) {
  try {
    return await readFile(path, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

async function writeOrCheck(path, expected, label) {
  if (!check) {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, expected, 'utf8');
    console.log(`generated ${path}`);
    return;
  }
  const existing = await readExisting(path);
  if (existing === expected) {
    console.log(`${label} is up to date: ${path}`);
    return;
  }
  if (existing === null) {
    console.error(`ERROR: ${label} is missing: ${path}`);
  } else {
    console.error(
      `ERROR: ${label} drifted from the TypeScript/Zod protocol registry; run pnpm gen:protocol to regenerate: ${path}`,
    );
  }
  process.exitCode = 1;
}

await writeOrCheck(rustPath, rustOut, 'generated Rust bindings');
await writeOrCheck(jsonPath, jsonGolden, 'generated JSON golden manifest');
await writeOrCheck(fixturesPath, jsonFixtures, 'generated wire fixtures');

if (process.exitCode === undefined || process.exitCode === 0) {
  console.log(`manifest fingerprint: ${manifest.manifestFingerprint}`);
}
