import { describe, expect, it } from 'vitest';

import { ProtocolCallError } from '../protocol/errors';
import { emptyCanvasDoc, openTile, serializeCanvasDoc, type CanvasDoc } from './split-tree';
import {
  hostTaskLayoutGateway,
  loadTaskLayout,
  saveTaskLayout,
  type TaskLayoutGateway,
} from './task-layout-persistence';

function memoryGateway(records: Map<string, { json: string; revision: number }>) {
  return {
    async load(taskId: string) {
      const record = records.get(taskId);
      return record === undefined
        ? ({ ok: true, layoutJson: null, revision: 0 } as const)
        : ({ ok: true, layoutJson: record.json, revision: record.revision } as const);
    },
    async save(taskId: string, layoutJson: string, expectedRevision?: number) {
      const current = records.get(taskId);
      if (
        current !== undefined &&
        expectedRevision !== undefined &&
        expectedRevision !== current.revision
      ) {
        return { ok: false, reason: 'conflict', message: 'revision conflict' } as const;
      }
      records.set(taskId, { json: layoutJson, revision: (current?.revision ?? 0) + 1 });
      return { ok: true, revision: records.get(taskId)!.revision } as const;
    },
  } satisfies TaskLayoutGateway;
}

const sampleDoc: CanvasDoc = openTile(emptyCanvasDoc(), {
  id: 'tile-1',
  entityId: 'epic-1',
  kind: 'chat',
});

describe('task layout persistence', () => {
  it('round-trips a document through the Host record', async () => {
    const records = new Map<string, { json: string; revision: number }>();
    const gateway = memoryGateway(records);

    const first = await loadTaskLayout(gateway, 'task-1');
    expect(first.status).toBe('missing');
    expect(first.doc).toBeNull();
    expect(first.revision).toBe(0);

    const saved = await saveTaskLayout(gateway, 'task-1', sampleDoc, first.revision || undefined);
    expect(saved).toEqual({ status: 'saved', revision: 1 });

    const reloaded = await loadTaskLayout(gateway, 'task-1');
    expect(reloaded.status).toBe('loaded');
    expect(reloaded.doc).toEqual(sampleDoc);
    expect(reloaded.revision).toBe(1);
  });

  it('reports conflicts from stale revision guards', async () => {
    const records = new Map<string, { json: string; revision: number }>();
    const gateway = memoryGateway(records);
    await saveTaskLayout(gateway, 'task-1', sampleDoc, undefined);

    const stale = await saveTaskLayout(gateway, 'task-1', sampleDoc, 0);
    expect(stale).toEqual({ status: 'conflict' });

    // A reload reveals the winning revision; the retry then succeeds.
    const fresh = await loadTaskLayout(gateway, 'task-1');
    expect(fresh.revision).toBe(1);
    const retried = await saveTaskLayout(gateway, 'task-1', sampleDoc, fresh.revision);
    expect(retried).toEqual({ status: 'saved', revision: 2 });
  });

  it('degrades to session-only state when the Host cannot serve layouts', async () => {
    const gateway: TaskLayoutGateway = {
      async load() {
        return { ok: false, reason: 'unavailable', message: 'host unreachable' };
      },
      async save() {
        return { ok: false, reason: 'unavailable', message: 'host unreachable' };
      },
    };
    const loaded = await loadTaskLayout(gateway, 'task-1');
    expect(loaded.status).toBe('unavailable');
    expect(loaded.doc).toBeNull();

    const saved = await saveTaskLayout(gateway, 'task-1', sampleDoc, undefined);
    expect(saved.status).toBe('unavailable');
  });

  it('maps FAILED_PRECONDITION answers to conflicts in the production gateway', async () => {
    let failNextSave = false;
    const client = {
      async call(methodName: string) {
        if (methodName === 'task.layout.get') {
          return { revision: 3 };
        }
        if (failNextSave) {
          throw new ProtocolCallError({
            code: 'FAILED_PRECONDITION',
            message: 'layout write conflict',
            layer: 'host',
          });
        }
        return { revision: 4 };
      },
    };
    const gateway = hostTaskLayoutGateway(() => client);

    const loaded = await gateway.load('task-1');
    expect(loaded).toEqual({ ok: true, layoutJson: null, revision: 3 });

    const saved = await gateway.save('task-1', '{}', 3);
    expect(saved).toEqual({ ok: true, revision: 4 });

    failNextSave = true;
    const conflicted = await saveTaskLayout(gateway, 'task-1', sampleDoc, 4);
    expect(conflicted).toEqual({ status: 'conflict' });
  });

  it('degrades typed host failures to unavailable in the production gateway', async () => {
    const client = {
      call: () =>
        Promise.reject(
          new ProtocolCallError({
            code: 'NOT_FOUND',
            message: 'unknown route',
            layer: 'host',
          }),
        ),
    };
    const gateway = hostTaskLayoutGateway(() => client);
    const loaded = await loadTaskLayout(gateway, 'task-1');
    expect(loaded.status).toBe('unavailable');
    const saved = await saveTaskLayout(gateway, 'task-1', sampleDoc, undefined);
    expect(saved.status).toBe('unavailable');
  });

  it('treats a corrupt persisted document as missing rather than fatal', async () => {
    const gateway = memoryGateway(
      new Map([['task-broken', { json: '{"version":9,"root":"garbage"}', revision: 4 }]]),
    );
    const loaded = await loadTaskLayout(gateway, 'task-broken');
    expect(loaded.status).toBe('loaded');
    expect(loaded.doc).toBeNull();
    expect(loaded.revision).toBe(4);
  });

  it('serializes deterministically for the wire', () => {
    const a = serializeCanvasDoc(sampleDoc);
    const b = serializeCanvasDoc(sampleDoc);
    expect(a).toBe(b);
    expect(() => JSON.parse(a)).not.toThrow();
  });
});
