import { act, cleanup, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { type ReactNode } from 'react';

import {
  leafNodes,
  openTile,
  setRatio,
  splitLeaf,
  serializeCanvasDoc,
  type CanvasDoc,
} from './split-tree';
import { loadTaskLayout, saveTaskLayout, type TaskLayoutGateway } from './task-layout-persistence';
import { useTaskLayout } from './use-task-layout';
import { resetShellForTests, useShellStore } from '../../state/shell-store';

function memoryGateway(): TaskLayoutGateway & { records: Map<string, string> } {
  const revisions = new Map<string, number>();
  const records = new Map<string, string>();
  return {
    records,
    async load(taskId) {
      const json = records.get(taskId);
      return json === undefined
        ? { ok: true, layoutJson: null, revision: 0 }
        : { ok: true, layoutJson: json, revision: revisions.get(taskId) ?? 1 };
    },
    async save(taskId, layoutJson, expectedRevision) {
      const current = revisions.get(taskId) ?? 0;
      if (expectedRevision !== undefined && expectedRevision !== current) {
        return { ok: false, reason: 'conflict', message: 'stale' };
      }
      revisions.set(taskId, current + 1);
      records.set(taskId, layoutJson);
      return { ok: true, revision: current + 1 };
    },
  };
}

type Probe = (binding: { doc: CanvasDoc | null; change(next: CanvasDoc): void }) => void;

function Harness({
  taskId,
  gateway,
  onProbe,
}: {
  taskId: string;
  gateway: TaskLayoutGateway;
  onProbe?: Probe;
}): ReactNode {
  const binding = useTaskLayout(taskId, gateway);
  onProbe?.(binding);
  if (binding.doc === null) {
    return <p data-testid="restoring">Restoring canvas…</p>;
  }
  const leaves = leafNodes(binding.doc.root);
  return (
    <div data-testid="canvas-summary">
      {leaves.map((leaf) => (
        <div key={leaf.id} data-testid="pane">
          {leaf.tiles.map((tile) => (
            <span key={tile.id}>{tile.kind}</span>
          ))}
        </div>
      ))}
    </div>
  );
}

describe('useTaskLayout persistence', () => {
  beforeEach(() => {
    resetShellForTests();
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('arranges a multi-level split, quits, relaunches, and restores exactly', async () => {
    const gateway = memoryGateway();

    // Session one: build (A | (B / C)) with distinct tiles.
    let captured: { doc: CanvasDoc | null; change(next: CanvasDoc): void } | undefined;
    const { unmount } = render(
      <Harness taskId="task-quit" gateway={gateway} onProbe={(b) => (captured = b)} />,
    );
    // Initial load resolves on microtasks even under fake timers.
    await act(async () => {
      await Promise.resolve();
    });
    expect(captured?.doc).not.toBeNull();

    let doc = openTile(captured!.doc!, { id: 't-a', entityId: 'epic-quit', kind: 'chat' });
    const firstLeafId = leafNodes(doc.root)[0]!.id;
    doc = splitLeaf(doc, firstLeafId, 'row').doc;
    const secondLeaf = leafNodes(doc.root).find((leaf) => leaf.id !== firstLeafId)!;
    doc = splitLeaf(doc, secondLeaf.id, 'column', {
      id: 't-b',
      entityId: 'epic-quit',
      kind: 'terminal',
    }).doc;
    doc = splitLeaf(doc, secondLeaf.id, 'column').doc;
    doc = openTile(doc, { id: 't-c', entityId: 'epic-quit', kind: 'artifact' });
    doc = setRatio(doc, doc.root.kind === 'split' ? doc.root.id : '', 0.37);

    act(() => {
      captured!.change(doc);
    });
    // Autosave debounce elapses -> Host record written.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700);
    });

    const persisted = gateway.records.get('task-quit');
    expect(persisted).toBeDefined();

    // Full quit: unmount everything, drop all in-memory state.
    unmount();
    resetShellForTests();

    // Relaunch: a brand-new harness restores from the record alone.
    let relaunched: { doc: CanvasDoc | null; change(next: CanvasDoc): void } | undefined;
    render(<Harness taskId="task-quit" gateway={gateway} onProbe={(b) => (relaunched = b)} />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(relaunched?.doc).not.toBeNull();
    expect(serializeCanvasDoc(relaunched!.doc!)).toBe(persisted);

    // Pixel-exact: every pane and tile binding survives, including an
    // intentionally empty one, in the original order and ratio.
    const leavesAfter = leafNodes(relaunched!.doc!.root);
    expect(leavesAfter).toHaveLength(4);
    expect(leavesAfter.flatMap((leaf) => leaf.tiles.map((tile) => tile.kind)).sort()).toEqual([
      'artifact',
      'chat',
      'terminal',
    ]);
    expect((relaunched!.doc!.root as { ratio: number }).ratio).toBe(0.37);
  });

  it('keeps working session-only when the Host cannot serve layouts', async () => {
    const gateway: TaskLayoutGateway = {
      async load() {
        return { ok: false, reason: 'unavailable', message: 'down' };
      },
      async save() {
        return { ok: false, reason: 'unavailable', message: 'down' };
      },
    };
    let captured: { doc: CanvasDoc | null; change(next: CanvasDoc): void } | undefined;
    render(<Harness taskId="task-offline" gateway={gateway} onProbe={(b) => (captured = b)} />);
    await act(async () => {
      await Promise.resolve();
    });

    // An empty canvas seeded locally; edits work without any persistence.
    expect(captured?.doc).not.toBeNull();
    const next = openTile(captured!.doc!, { id: 't-x', entityId: 'e', kind: 'chat' });
    act(() => {
      captured!.change(next);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700);
    });
    expect(useShellStore.getState().canvases['task-offline']).toBeDefined();
  });

  it('retries unguarded after a revision conflict so edits converge', async () => {
    const base = memoryGateway();
    let guarded = true;
    const gateway: TaskLayoutGateway = {
      load: base.load,
      async save(taskId, layoutJson, expectedRevision) {
        if (guarded && expectedRevision !== undefined) {
          return { ok: false, reason: 'conflict', message: 'someone else wrote' };
        }
        return base.save(taskId, layoutJson, expectedRevision);
      },
    };

    let captured: { doc: CanvasDoc | null; change(next: CanvasDoc): void } | undefined;
    render(<Harness taskId="task-conflict" gateway={gateway} onProbe={(b) => (captured = b)} />);
    await act(async () => {
      await Promise.resolve();
    });
    const edited = openTile(captured!.doc!, { id: 't-y', entityId: 'e', kind: 'terminal' });
    act(() => {
      captured!.change(edited);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700); // first attempt conflicts
      await vi.advanceTimersByTimeAsync(300); // retry lands unguarded
    });
    expect(base.records.get('task-conflict')).toContain('terminal');
    guarded = false;
  });

  it('load and save round-trip through the gateway unchanged', async () => {
    const gateway = memoryGateway();
    const seed = openTile(
      openTile(openTile({ ...emptyDoc() }, tileOf('a')), tileOf('b')),
      tileOf('c'),
    );
    await saveTaskLayout(gateway, 'task-roundtrip', seed, undefined);
    const loaded = await loadTaskLayout(gateway, 'task-roundtrip');
    expect(loaded.status).toBe('loaded');
    expect(serializeCanvasDoc(loaded.doc!)).toBe(serializeCanvasDoc(seed));
  });
});

// -- helpers ----------------------------------------------------------------

function emptyDoc(): CanvasDoc {
  return {
    version: 1,
    root: { kind: 'leaf', id: 'seed-root', tiles: [], activeTileId: null },
    maximizedLeafId: null,
  };
}

function tileOf(id: string) {
  return { id, entityId: `entity-${id}`, kind: 'chat' as const };
}
