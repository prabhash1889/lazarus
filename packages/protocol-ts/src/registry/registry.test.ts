import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { describe, it } from 'node:test';
import { fileURLToPath } from 'node:url';

import { z } from 'zod';

import {
  METHODS,
  methodByName,
  snapshotManifest,
  snapshotMethod,
  fingerprintSchema,
  isFingerprint,
  isSchemaBackwardCompatible,
  jsonSchemaOf,
  validateManifestTransition,
  validateFrozenContractTransition,
  declareBridge,
  findBridge,
  bridgeFor,
  bridgedOlderMinors,
  isInteroperable,
  requiredBridgeCoverageViolations,
  resolveMethodSupport,
  adaptNewerToOlder,
  releasedFloorGaps,
  releasedFloorSnapshot,
  RELEASED_FLOOR,
  PERSISTENCE_RECORDS,
  snapshotPersistenceRegistry,
} from './index.ts';
import type { FrozenContractMethod, MethodDefinition } from './index.ts';
import { EventFrameSchema } from './schemas/system.ts';
import { ErrorCodeSchema, ProtocolErrorSchema } from './schemas/common.ts';

const taskList = (): MethodDefinition => ({ ...methodByName('task.list')! }) as MethodDefinition;

const frozenOf = (method: MethodDefinition): FrozenContractMethod => ({
  name: method.name,
  version: method.version,
  requestSchema: jsonSchemaOf(method.request),
  responseSchema: jsonSchemaOf(method.response),
});

const loadReleasedBaseline = (): FrozenContractMethod[] =>
  (
    JSON.parse(
      readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../../released-contract.json'), {
        encoding: 'utf8',
      }),
    ) as { methods: FrozenContractMethod[] }
  ).methods;

describe('method manifest', () => {
  it('declares every Phase 1 method with request/response schemas and versions', () => {
    const names = METHODS.map((m) => m.name).sort();
    assert.deepEqual(names, [
      'system.getInfo',
      'system.health',
      'system.subscribeEvents',
      'task.list',
      'workspace.list',
    ]);
    for (const method of METHODS) {
      assert.ok(method.version.major >= 1);
      assert.ok(method.version.minor >= 0);
      // Schemas must actually parse representative payloads.
      void method.request;
      void method.response;
    }
    const health = methodByName('system.health')!;
    assert.equal(
      (health.response.parse({ status: 'SERVING' }) as { status: string }).status,
      'SERVING',
    );
    const tasks = methodByName('task.list')!;
    assert.equal((tasks.request.parse({ pageSize: 10 }) as { pageSize: number }).pageSize, 10);
    const workspaces = methodByName('workspace.list')!;
    assert.ok(
      Array.isArray(
        (workspaces.response.parse({ workspaces: [] }) as { workspaces: unknown[] }).workspaces,
      ),
    );
    const info = methodByName('system.getInfo')!;
    const parsedInfo = info.response.parse({
      hostVersion: '1.0.0',
      capabilities: { 'task.cancel': true },
    }) as { hostVersion: string; capabilities: Record<string, boolean> };
    assert.equal(parsedInfo.hostVersion, '1.0.0');
    assert.equal(parsedInfo.capabilities['task.cancel'], true);
  });

  it('fingerprints are deterministic and schema-sensitive', () => {
    const first = snapshotManifest();
    const second = snapshotManifest();
    assert.equal(first.manifestFingerprint, second.manifestFingerprint);
    for (const m of first.methods) {
      assert.ok(isFingerprint(m.requestFingerprint));
      assert.ok(isFingerprint(m.responseFingerprint));
    }
    const widened = z.object({
      id: z.string(),
      title: z.string(),
      status: z.string(),
    });
    const original = z.object({
      id: z.string(),
      title: z.string(),
      status: z.enum(['PENDING', 'RUNNING']),
    });
    assert.notEqual(fingerprintSchema(original), fingerprintSchema(widened));
  });
});

describe('event frame wire contract', () => {
  // Representative frames exactly as crates/host/src/events.rs serializes
  // them (camelCase, `type`-tagged SSE data payloads).
  it('accepts the exact outage/snapshot/live Host frames', () => {
    const outage = EventFrameSchema.parse(JSON.parse('{"type":"outage","outageId":"outage-1"}'));
    assert.deepEqual(outage, { type: 'outage', outageId: 'outage-1' });

    const snapshot = EventFrameSchema.parse(
      JSON.parse(
        '{"type":"snapshot","workspaces":[{"id":"w-1","name":"Main"}],"tasks":[{"id":"t-1","workspaceId":"w-1","title":"Ship it"}]}',
      ),
    );
    assert.deepEqual(snapshot, {
      type: 'snapshot',
      workspaces: [{ id: 'w-1', name: 'Main' }],
      tasks: [{ id: 't-1', workspaceId: 'w-1', title: 'Ship it' }],
    });
    assert.equal(
      EventFrameSchema.safeParse(JSON.parse('{"type":"snapshot","workspaces":[],"tasks":[]}'))
        .success,
      true,
    );

    const live = EventFrameSchema.parse(JSON.parse('{"type":"live","sequence":7}'));
    assert.deepEqual(live, { type: 'live', sequence: 7 });
  });

  it('rejects the obsolete workspace.changed/task.changed frames', () => {
    for (const raw of [
      '{"type":"workspace.changed","sequence":1}',
      '{"type":"task.changed","sequence":2}',
    ]) {
      assert.equal(
        EventFrameSchema.safeParse(JSON.parse(raw)).success,
        false,
        `must reject ${raw}`,
      );
    }
  });

  it('rejects malformed live sequences', () => {
    for (const raw of [
      '{"type":"live"}',
      '{"type":"live","sequence":-1}',
      '{"type":"live","sequence":1.5}',
    ]) {
      assert.equal(
        EventFrameSchema.safeParse(JSON.parse(raw)).success,
        false,
        `must reject ${raw}`,
      );
    }
  });

  it('is registered as the system.subscribeEvents response schema', () => {
    const subscribe = methodByName('system.subscribeEvents')!;
    assert.ok(isFingerprint(fingerprintSchema(subscribe.response)));
    assert.equal(fingerprintSchema(subscribe.response), fingerprintSchema(EventFrameSchema));
  });
});

describe('error envelope wire contract', () => {
  // The exact 412 body crates/host/src/services.rs serves when a required
  // method is missing from the peer manifest (Incompatibility::RequiredMissing).
  it('parses the actual Host INCOMPATIBLE_METHOD_MANIFEST rejection body', () => {
    const raw =
      '{"code":"INCOMPATIBLE_METHOD_MANIFEST","message":"required method \\"task.list\\" missing from peer manifest"}';
    const parsed = ProtocolErrorSchema.parse(JSON.parse(raw));
    assert.equal(parsed.code, 'INCOMPATIBLE_METHOD_MANIFEST');
    assert.equal(parsed.message, 'required method "task.list" missing from peer manifest');
    assert.ok(ErrorCodeSchema.safeParse('INCOMPATIBLE_METHOD_MANIFEST').success);
    assert.ok(!ErrorCodeSchema.safeParse('MANIFEST_MISMATCH').success);
  });
});

describe('additive-minor validation', () => {
  it('accepts an additive minor (new optional field)', () => {
    const released = taskList();
    const candidate: MethodDefinition = {
      ...released,
      version: { major: 1, minor: 3 },
      request: z.object({
        pageSize: z.number().int().min(1).max(100).optional(),
        cursor: z.string().optional(),
        statusFilter: z.string().optional(),
      }),
    };
    assert.deepEqual(validateManifestTransition([released], [candidate]), []);
  });

  it('rejects a breaking minor (newly required field)', () => {
    const released = taskList();
    const candidate: MethodDefinition = {
      ...released,
      version: { major: 1, minor: 3 },
      response: z.object({
        tasks: z.array(z.object({ id: z.string(), title: z.string(), status: z.string() })),
        pagination: z.object({ nextCursor: z.string() }),
      }),
    };
    const violations = validateManifestTransition([released], [candidate]);
    assert.equal(violations.length, 1);
    assert.equal(violations[0]!.rule, 'breaking-schema-change');
  });

  it('rejects minor regressions, major changes, and removals', () => {
    const released = taskList();

    // Same major, same minor, identical schemas: a no-op transition.
    const noOp = validateManifestTransition([released], [{ ...released }]);
    assert.deepEqual(noOp, []);

    // A minor that goes backwards cannot replace the released one.
    const released13: MethodDefinition = { ...released, version: { major: 1, minor: 3 } };
    const regressed = validateManifestTransition([released13], [taskList()]);
    assert.equal(regressed.length, 1);
    assert.equal(regressed[0]!.rule, 'minor-regressed');

    const majorChanged = validateManifestTransition(
      [released],
      [{ ...released, version: { major: 2, minor: 0 }, request: z.object({}) }],
    );
    assert.equal(majorChanged.length, 1);
    assert.equal(majorChanged[0]!.rule, 'major-changed');

    const removed = validateManifestTransition([released], []);
    assert.equal(removed.length, 1);
    assert.equal(removed[0]!.rule, 'method-removed');
  });

  it('structural check distinguishes widened enums from narrowed ones', () => {
    const old = { type: 'string', enum: ['A', 'B'] };
    const widened = { type: 'string', enum: ['A', 'B', 'C'] };
    const narrowed = { type: 'string', enum: ['A'] };
    assert.equal(isSchemaBackwardCompatible(old, widened), true);
    assert.equal(isSchemaBackwardCompatible(old, narrowed), false);
  });

  it('rejects acceptance constraints introduced only by the candidate', () => {
    // The exact probes from the release-gate review: each of these breaking
    // transitions used to be classified as backward compatible.
    assert.equal(
      isSchemaBackwardCompatible({ type: 'string' }, { type: 'string', enum: ['A', 'B'] }),
      false,
      'unconstrained string -> enum',
    );
    assert.equal(
      isSchemaBackwardCompatible({ type: 'number' }, { type: 'number', minimum: 0 }),
      false,
      'unconstrained number -> minimum',
    );
    assert.equal(
      isSchemaBackwardCompatible({ type: 'string' }, { type: 'string', maxLength: 5 }),
      false,
      'unconstrained string -> maxLength',
    );
    assert.equal(
      isSchemaBackwardCompatible({ type: 'array' }, { type: 'array', items: { type: 'string' } }),
      false,
      'unconstrained array -> items',
    );
    assert.equal(
      isSchemaBackwardCompatible({ type: 'number' }, { type: 'number', maximum: 100 }),
      false,
      'unconstrained number -> maximum',
    );
    assert.equal(
      isSchemaBackwardCompatible({ type: 'string' }, { type: 'string', minLength: 1 }),
      false,
      'unconstrained string -> minLength',
    );
    assert.equal(
      isSchemaBackwardCompatible({ type: 'string' }, { type: 'string', const: 'A' }),
      false,
      'unconstrained string -> const',
    );
    // The same restriction nested inside a previously unconstrained property.
    assert.equal(
      isSchemaBackwardCompatible(
        { type: 'object', properties: { status: { type: 'string' } } },
        { type: 'object', properties: { status: { type: 'string', enum: ['OK'] } } },
      ),
      false,
    );
    // Dropping a released constraint is still genuine widening.
    assert.equal(
      isSchemaBackwardCompatible({ type: 'string', maxLength: 5 }, { type: 'string' }),
      true,
    );
  });

  it('structural check distinguishes widened unions from narrowed ones', () => {
    const old = {
      oneOf: [
        {
          type: 'object',
          properties: { kind: { type: 'string', const: 'A' } },
          required: ['kind'],
        },
        {
          type: 'object',
          properties: { kind: { type: 'string', const: 'B' } },
          required: ['kind'],
        },
      ],
    };
    const widened = { anyOf: [...old.oneOf, { type: 'null' }] };
    const narrowed = { oneOf: [old.oneOf[0]] };
    assert.equal(isSchemaBackwardCompatible(old, widened), true);
    assert.equal(isSchemaBackwardCompatible(old, narrowed), false);
  });

  it('rejects const changes and fail-closes unknown keywords', () => {
    const old = {
      type: 'object',
      properties: { type: { type: 'string', const: 'live' } },
      required: ['type'],
    };
    const changedConst = {
      type: 'object',
      properties: { type: { type: 'string', const: 'snapshot' } },
      required: ['type'],
    };
    assert.equal(isSchemaBackwardCompatible(old, changedConst), false);

    const narrowedByPattern = {
      type: 'object',
      properties: { type: { type: 'string', const: 'live' } },
      required: ['type'],
      pattern: '^l',
    };
    assert.equal(isSchemaBackwardCompatible(old, narrowedByPattern), false);
  });

  it('rejects an array item type change (string -> number)', () => {
    const released = taskList();
    const candidate: MethodDefinition = {
      ...released,
      version: { major: 1, minor: 3 },
      response: z.object({
        tasks: z.array(z.object({ id: z.string(), title: z.string(), status: z.number() })),
        pagination: z.object({ nextCursor: z.string().optional() }).optional(),
      }),
    };
    const violations = validateManifestTransition([released], [candidate]);
    assert.equal(violations.length, 1);
    assert.equal(violations[0]!.rule, 'breaking-schema-change');
  });

  it('requires a minor bump for any schema change, even additive ones', () => {
    const released = taskList();
    // Same version, purely additive optional field: still rejected.
    const sameVersion: MethodDefinition = {
      ...released,
      request: z.object({
        pageSize: z.number().int().min(1).max(100).optional(),
        cursor: z.string().optional(),
        statusFilter: z.string().optional(),
        tagFilter: z.string().optional(),
      }),
    };
    const sameVersionViolations = validateManifestTransition([released], [sameVersion]);
    assert.equal(sameVersionViolations.length, 1);
    assert.equal(sameVersionViolations[0]!.rule, 'minor-bump-required');

    // The identical change with a minor bump is accepted.
    const bumped: MethodDefinition = { ...sameVersion, version: { major: 1, minor: 3 } };
    assert.deepEqual(validateManifestTransition([released], [bumped]), []);
  });

  it('applies request and response compatibility in the correct direction', () => {
    assert.equal(isSchemaBackwardCompatible({}, { type: 'string' }), false);
    assert.equal(
      isSchemaBackwardCompatible({}, { oneOf: [{ type: 'string' }, { type: 'number' }] }),
      false,
    );

    const released: MethodDefinition = {
      name: 'test.variance',
      kind: 'unary',
      version: { major: 1, minor: 0 },
      request: z.object({}),
      response: z.object({ status: z.enum(['A']), score: z.number().min(0) }),
      optional: false,
    };
    const widenedResponse: MethodDefinition = {
      ...released,
      version: { major: 1, minor: 1 },
      response: z.object({ status: z.enum(['A', 'B']), score: z.number().min(-1) }),
    };
    assert.ok(
      validateManifestTransition([released], [widenedResponse]).some(
        (violation) => violation.rule === 'breaking-schema-change',
      ),
    );

    const additiveResponse: MethodDefinition = {
      ...released,
      version: { major: 1, minor: 1 },
      response: z.object({
        status: z.enum(['A']),
        score: z.number().min(0),
        note: z.string().optional(),
      }),
    };
    assert.deepEqual(validateManifestTransition([released], [additiveResponse]), []);
  });
});

describe('frozen released contract', () => {
  it('current registry is a valid additive-minor transition from the baseline', () => {
    const violations = validateFrozenContractTransition(
      loadReleasedBaseline(),
      METHODS.map(frozenOf),
    );
    assert.deepEqual(violations, []);
  });

  it('rejects a same-version schema edit even though generation would rewrite outputs', () => {
    // Hermetic: the baseline is derived from the live method definition, so
    // this holds no matter which minor the registry currently serves.
    const baseline = [frozenOf(taskList())];
    const candidate: FrozenContractMethod = {
      ...baseline[0]!,
      requestSchema: jsonSchemaOf(
        z.object({
          pageSize: z.number().int().min(1).max(100).optional(),
          cursor: z.string().optional(),
          tagFilter: z.string().optional(),
        }),
      ),
    };
    const violations = validateFrozenContractTransition(baseline, [candidate]);
    assert.equal(violations.length, 1);
    assert.equal(violations[0]!.rule, 'minor-bump-required');
  });

  it('reviewer probe: narrowing EventFrame variants fails the compatibility gate', () => {
    // The exact probe from the review: collapse the discriminated union to
    // one variant and bump the minor; this must not pass as additive.
    const subscribe = methodByName('system.subscribeEvents')!;
    const baseline = [frozenOf(subscribe)];
    const narrowedFrames = z.discriminatedUnion('type', [
      z.object({ type: z.literal('live'), sequence: z.number().int().nonnegative() }),
    ]);
    const candidate: FrozenContractMethod = {
      ...baseline[0]!,
      version: { major: subscribe.version.major, minor: subscribe.version.minor + 1 },
      responseSchema: jsonSchemaOf(narrowedFrames),
    };
    const violations = validateFrozenContractTransition(baseline, [candidate]);
    assert.ok(violations.length >= 1);
    assert.ok(
      violations.some(
        (v) => v.rule === 'breaking-schema-change' && v.method === 'system.subscribeEvents',
      ),
    );
  });

  it('rejects a widened EventFrame union because responses only add optional fields', () => {
    const subscribe = methodByName('system.subscribeEvents')!;
    const baseline = [frozenOf(subscribe)];
    const widenedFrames = z.discriminatedUnion('type', [
      z.object({ type: z.literal('outage'), outageId: z.string().min(1) }),
      z.object({
        type: z.literal('snapshot'),
        workspaces: z.array(z.object({ id: z.string(), name: z.string() })),
        tasks: z.array(z.object({ id: z.string(), workspaceId: z.string(), title: z.string() })),
      }),
      z.object({ type: z.literal('live'), sequence: z.number().int().nonnegative() }),
      z.object({ type: z.literal('heartbeat'), atMs: z.number().int().nonnegative() }),
    ]);
    const candidate: FrozenContractMethod = {
      ...baseline[0]!,
      version: { major: subscribe.version.major, minor: subscribe.version.minor + 1 },
      responseSchema: jsonSchemaOf(widenedFrames),
    };
    assert.ok(
      validateFrozenContractTransition(baseline, [candidate]).some(
        (violation) => violation.rule === 'breaking-schema-change',
      ),
    );
  });

  it('the production registry keeps bridge coverage for every advanced released minor', () => {
    assert.deepEqual(requiredBridgeCoverageViolations(loadReleasedBaseline(), [...METHODS]), []);
  });
});

describe('bridges', () => {
  it('executes declarative steps downgrading newer payloads toward an older minor', () => {
    declareBridge({
      method: 'test.method',
      older: { major: 1, minor: 0 },
      newer: { major: 1, minor: 2 },
      steps: [{ op: 'omitResponseFields', fields: ['extra', 'nested.inner'] }],
    });

    const bridge = findBridge('test.method', 0)!;
    const local = localMethod('test.method');
    assert.deepEqual(bridgeFor(local, { major: 1, minor: 0 }), bridge);
    assert.deepEqual(adaptNewerToOlder(bridge, { id: 'a', extra: true }), { id: 'a' });
    // Only top-level fields are touched; everything else passes through.
    assert.deepEqual(adaptNewerToOlder(bridge, { nested: { inner: 1 }, other: [{ extra: 2 }] }), {
      nested: { inner: 1 },
      other: [{ extra: 2 }],
    });
    // Non-object payloads pass through untouched.
    assert.deepEqual(adaptNewerToOlder(bridge, [1, 2]), [1, 2]);

    assert.deepEqual(bridgedOlderMinors(local), [0]);
    assert.equal(isInteroperable(local, { major: 1, minor: 0 }), true);
    // A numerically plausible but undeclared older minor stays unusable.
    assert.equal(isInteroperable(local, { major: 1, minor: 1 }), false);
    assert.equal(isInteroperable(local, { major: 1, minor: 5 }), true);
    assert.deepEqual(bridgedOlderMinors(localMethodAt('test.method', 3)), []);
  });

  it('refuses malformed bridge declarations', () => {
    assert.throws(
      () =>
        declareBridge({
          method: 'test.crossMajor',
          older: { major: 1, minor: 0 },
          newer: { major: 2, minor: 0 },
          steps: [],
        }),
      /one method major/,
    );
    assert.throws(
      () =>
        declareBridge({
          method: 'test.notOlder',
          older: { major: 1, minor: 1 },
          newer: { major: 1, minor: 1 },
          steps: [],
        }),
      /strictly older/,
    );
    assert.throws(
      () =>
        declareBridge({
          method: 'test.regressed',
          older: { major: 1, minor: 3 },
          newer: { major: 1, minor: 2 },
          steps: [],
        }),
      /strictly older/,
    );
  });

  it('the production task.list bridge keeps the released floor interoperable', () => {
    const taskList = methodByName('task.list')!;
    assert.equal(taskList.version.minor, 2);
    assert.deepEqual(bridgedOlderMinors(taskList), [0]);
    assert.equal(isInteroperable(taskList, { major: 1, minor: 0 }), true);
    // Minor 1 was never published, so no bridge can declare it.
    assert.equal(isInteroperable(taskList, { major: 1, minor: 1 }), false);
    assert.equal(isInteroperable(taskList, { major: 1, minor: 2 }), true);
    assert.equal(isInteroperable(taskList, { major: 1, minor: 4 }), true);

    const bridge = findBridge('task.list', 0)!;
    assert.deepEqual(adaptNewerToOlder(bridge, { tasks: [], servedAtUnixMs: 42 }), { tasks: [] });
    // Unrelated methods have no bridges at all.
    assert.deepEqual(bridgedOlderMinors(methodByName('workspace.list')!), []);
  });
});

describe('required-method bridge coverage gate', () => {
  it('fails generation for an advanced required released minor without a bridge', () => {
    // No bridge is declared for this method: normal generation/check must
    // refuse the additive bump instead of 412-ing the released peer later.
    assert.deepEqual(
      requiredBridgeCoverageViolations(
        [{ name: 'gate.required', version: { major: 1, minor: 0 } }],
        [localMethodAt('gate.required', 3)],
      ),
      [
        {
          method: 'gate.required',
          rule: 'bridge-coverage-required',
          detail:
            "gate.required: required method's minor advanced 1.0 -> 1.3 without a declared bridge from " +
            'the released minor to the current version; released peers would be rejected at negotiation',
        },
      ],
    );
  });

  it('accepts coverage only when the bridge targets the current registry version', () => {
    declareBridge({
      method: 'gate.stale',
      older: { major: 1, minor: 0 },
      newer: { major: 1, minor: 5 },
      steps: [],
    });
    // The declared bridge points at 1.5 while the registry serves 1.2.
    assert.deepEqual(
      requiredBridgeCoverageViolations(
        [{ name: 'gate.stale', version: { major: 1, minor: 0 } }],
        [localMethodAt('gate.stale', 2)],
      ).length,
      1,
    );

    declareBridge({
      method: 'gate.covered',
      older: { major: 1, minor: 0 },
      newer: { major: 1, minor: 4 },
      steps: [],
    });
    assert.deepEqual(
      requiredBridgeCoverageViolations(
        [{ name: 'gate.covered', version: { major: 1, minor: 0 } }],
        [localMethodAt('gate.covered', 4)],
      ),
      [],
    );
  });

  it('does not blanket-require bridges for optional methods', () => {
    // Optional methods degrade via fallback/unsupported instead of failing
    // negotiation, so their minor may advance without a declared bridge.
    const optionalAdvanced: MethodDefinition = {
      ...localMethodAt('gate.optional', 2),
      optional: true,
      fallback: 'system.health',
    };
    assert.deepEqual(
      requiredBridgeCoverageViolations(
        [{ name: 'gate.optional', version: { major: 1, minor: 0 } }],
        [optionalAdvanced],
      ),
      [],
    );
  });

  it('ignores unchanged, regressed, and major-changed transitions here', () => {
    // Those cases are reported by the frozen-contract schema check; the
    // coverage gate only fires on forward minor advances within a major.
    assert.deepEqual(
      requiredBridgeCoverageViolations(
        [
          { name: 'gate.same', version: { major: 1, minor: 2 } },
          { name: 'gate.major', version: { major: 1, minor: 0 } },
        ],
        [
          localMethodAt('gate.same', 2),
          { ...localMethodAt('gate.major', 0), version: { major: 2, minor: 0 } },
        ],
      ),
      [],
    );
  });
});

function localMethod(name: string): MethodDefinition {
  return localMethodAt(name, 2);
}

function localMethodAt(name: string, minor: number): MethodDefinition {
  return {
    name,
    kind: 'unary',
    version: { major: 1, minor },
    request: z.object({}),
    response: z.object({ id: z.string() }),
    optional: false,
  };
}

describe('optional method support resolution', () => {
  it('degrades an unsupported optional method without affecting others', () => {
    const requiredHealth = methodByName('system.health')!;
    const optionalExperimental: MethodDefinition = {
      name: 'system.experimentalDiagnostics',
      kind: 'unary',
      version: { major: 1, minor: 0 },
      request: z.object({}),
      response: z.object({ ok: z.boolean() }),
      optional: true,
      fallback: 'system.health',
    };

    // Peer advertises nothing relevant.
    const peerVersions = new Map([['workspace.list', { major: 1, minor: 0 }]]);
    assert.equal(resolveMethodSupport(optionalExperimental, peerVersions), 'unsupported');
    // An unrelated missing method does not fail the supported ones.
    assert.equal(resolveMethodSupport(requiredHealth, peerVersions), 'unsupported');
    const withHealth = new Map([['system.health', { major: 1, minor: 3 }]]);
    assert.equal(resolveMethodSupport(requiredHealth, withHealth), 'supported');
    assert.equal(resolveMethodSupport(optionalExperimental, withHealth), 'fallback');
    assert.equal(
      resolveMethodSupport(
        optionalExperimental,
        new Map([['system.health', { major: 2, minor: 0 }]]),
      ),
      'unsupported',
    );

    const withTaskFallback: MethodDefinition = {
      ...optionalExperimental,
      fallback: 'task.list',
    };
    assert.equal(
      resolveMethodSupport(withTaskFallback, new Map([['task.list', { major: 1, minor: 0 }]])),
      'fallback',
    );
    assert.equal(
      resolveMethodSupport(withTaskFallback, new Map([['task.list', { major: 1, minor: 1 }]])),
      'unsupported',
    );

    const none = new Map<string, { major: number; minor: number }>();
    assert.equal(resolveMethodSupport(requiredHealth, none), 'unsupported');
    assert.notEqual(
      resolveMethodSupport(
        optionalExperimental,
        new Map([['system.experimentalDiagnostics', { major: 2, minor: 0 }]]),
      ),
      'supported',
    );
  });
});

describe('released floor', () => {
  it('is fully present in the current registry at or above floor versions', () => {
    assert.deepEqual(releasedFloorGaps(), []);
    assert.equal(RELEASED_FLOOR.size, 5);
  });

  it('snapshots to exactly the floor methods', () => {
    const snapshot = releasedFloorSnapshot();
    assert.deepEqual(snapshot.methods.map((m) => m.name).sort(), [
      'system.getInfo',
      'system.health',
      'system.subscribeEvents',
      'task.list',
      'workspace.list',
    ]);
  });
});

describe('persistence registry skeleton', () => {
  it('versions independently from RPC methods and snapshots deterministically', () => {
    assert.deepEqual(PERSISTENCE_RECORDS, []);
    const snap = snapshotPersistenceRegistry();
    assert.equal(snap.namespace, 'lazarus.persistence.v1');
    assert.equal(snap.registryFingerprint, snapshotPersistenceRegistry().registryFingerprint);

    // A record's version space is its own; nothing here reads METHOD data.
    const records = [
      {
        name: 'task.record',
        version: { major: 1, minor: 0 },
        schema: z.object({ id: z.string(), title: z.string() }),
      },
    ];
    const withRecord = snapshotPersistenceRegistry(records);
    assert.equal(withRecord.records.length, 1);
    assert.ok(isFingerprint(withRecord.records[0]!.schemaFingerprint));
    assert.notEqual(withRecord.registryFingerprint, snap.registryFingerprint);
  });
});

describe('manifest snapshots', () => {
  it('sort methods deterministically by name', () => {
    const manifest = snapshotManifest();
    const names = manifest.methods.map((m) => m.name);
    const sorted = [...names].sort();
    assert.deepEqual(names, sorted);
    const single = snapshotMethod(METHODS[0]!);
    assert.ok(isFingerprint(single.requestFingerprint));
  });
});
