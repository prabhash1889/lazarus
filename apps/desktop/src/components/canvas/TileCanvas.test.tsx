import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';

import { useState, type ReactNode } from 'react';

import {
  emptyCanvasDoc,
  leafNodes,
  openTile,
  splitLeaf,
  type CanvasDoc,
} from '../../lib/canvas/split-tree';
import { resetEpicsForTests, useEpicsStore } from '../../state/epics-store';
import { TileCanvas } from './TileCanvas';
import { TilePlaceholder } from './TilePlaceholder';

const TILE_MIME = 'application/x-lazarus-tile-id';

function Harness({
  initial,
  onChange,
}: {
  initial: CanvasDoc;
  onChange?: (doc: CanvasDoc) => void;
}): ReactNode {
  const [doc, setDoc] = useState(initial);
  return (
    <div>
      <TileCanvas
        doc={doc}
        onChange={(next) => {
          setDoc(next);
          onChange?.(next);
        }}
        renderTile={(binding) => <TilePlaceholder binding={binding} />}
        createTile={(kind) => ({
          id: `tile-${kind}-${Math.random().toString(36).slice(2, 8)}`,
          entityId: 'epic-1',
          kind,
        })}
      />
      <span data-testid="doc-snapshot">{JSON.stringify(doc)}</span>
    </div>
  );
}

function makeEpic(): void {
  useEpicsStore.getState().putEpic({ id: 'epic-1', title: 'Launch pad', createdAt: 0 });
}

function currentDoc(): CanvasDoc {
  const snapshot = screen.getByTestId('doc-snapshot').textContent ?? '{}';
  return JSON.parse(snapshot) as CanvasDoc;
}

describe('tile canvas', () => {
  afterEach(() => {
    cleanup();
    resetEpicsForTests();
  });

  it('renders the empty-canvas state and opens a tile into the first pane', async () => {
    const user = userEvent.setup();
    makeEpic();
    render(<Harness initial={emptyCanvasDoc()} />);

    expect(screen.getByTestId('tile-empty-state')).toBeTruthy();
    await user.click(screen.getByTestId('open-chat'));

    const doc = currentDoc();
    expect(doc.root.kind === 'leaf' ? doc.root.tiles : []).toHaveLength(1);
    expect(screen.getByRole('tab', { selected: true }).textContent).toContain('chat');
    expect(screen.getByText(/Chat agent sessions arrive/)).toBeTruthy();
  });

  it('closes tiles without touching the backing entity', async () => {
    const user = userEvent.setup();
    makeEpic();
    let doc = openTile(emptyCanvasDoc(), { id: 't-chat', entityId: 'epic-1', kind: 'chat' });
    doc = splitLeaf(doc, leafNodes(doc.root)[0]!.id, 'row', {
      id: 't-term',
      entityId: 'epic-1',
      kind: 'terminal',
    }).doc;
    render(<Harness initial={doc} />);

    await user.click(screen.getByTestId('close-t-term'));

    // Tile gone; entity intact and the tree collapsed back to one pane.
    expect(screen.queryByTestId('tile-tab-t-term')).toBeNull();
    expect(useEpicsStore.getState().epics['epic-1']).not.toBeUndefined();
    expect(currentDoc().root.kind).toBe('leaf');
  });

  it('keeps two tiles bound to the same entity in sync', async () => {
    const user = userEvent.setup();
    makeEpic();
    let doc = openTile(emptyCanvasDoc(), {
      id: 'tile-a',
      entityId: 'epic-1',
      kind: 'chat',
    });
    doc = openTile(doc, { id: 'tile-b', entityId: 'epic-1', kind: 'artifact' });
    render(<Harness initial={doc} />);

    // Only tile-b is active; rename there and commit on blur...
    const inputB = screen.getByTestId('tile-title-input-tile-b') as HTMLInputElement;
    await user.clear(inputB);
    await user.type(inputB, 'Renamed epic');
    await user.click(screen.getByTestId('tile-tab-tile-a'));
    expect(useEpicsStore.getState().epics['epic-1']!.title).toBe('Renamed epic');

    // ...then focus tile-a and observe the committed shared state.
    const inputA = screen.getByTestId('tile-title-input-tile-a') as HTMLInputElement;
    expect(inputA.value).toBe('Renamed epic');
  });

  it('moves tiles between panes via drag and drop', () => {
    makeEpic();
    let doc = openTile(emptyCanvasDoc(), { id: 't-move', entityId: 'epic-1', kind: 'chat' });
    const sourceLeafId = leafNodes(doc.root)[0]!.id;
    doc = splitLeaf(doc, sourceLeafId, 'row').doc;
    const targetLeaf = leafNodes(doc.root).find((leaf) => leaf.id !== sourceLeafId)!;

    const dataTransfer = {
      types: [TILE_MIME],
      getData: (type: string) => (type === TILE_MIME ? 't-move' : ''),
      setData: () => undefined,
      effectAllowed: 'move',
    };
    render(<Harness initial={doc} />);
    const targetPane = screen.getByTestId(`pane-${targetLeaf.id}`);
    const tab = screen.getByTestId('tile-tab-t-move');

    fireEvent.dragStart(tab, { dataTransfer });
    fireEvent.drop(targetPane, { dataTransfer });

    const after = currentDoc();
    expect(after).not.toEqual(doc);
    // Both panes were/are empty around the move, so the tree collapses to
    // a single pane that receives the tile.
    const leavesAfter = leafNodes(after.root);
    expect(leavesAfter.some((leaf) => leaf.tiles.some((t) => t.id === 't-move'))).toBe(true);
  });

  it('maximizes one pane, hides siblings, and restores back', async () => {
    const user = userEvent.setup();
    let doc = openTile(emptyCanvasDoc(), { id: 't1', entityId: 'e', kind: 'chat' });
    const leafId = leafNodes(doc.root)[0]!.id;
    doc = splitLeaf(doc, leafId, 'column').doc;
    const otherLeaf = leafNodes(doc.root).find((leaf) => leaf.id !== leafId)!;

    render(<Harness initial={doc} />);
    const canvas = screen.getByTestId('tile-canvas');
    expect(canvas.querySelectorAll('.tile-pane')).toHaveLength(2);

    await user.click(screen.getByTestId(`maximize-${leafId}`));
    expect(canvas.getAttribute('data-maximized')).toBe(leafId);
    // The maximized view replaces the whole canvas: exactly one pane, and
    // the hidden sibling's controls are unreachable while maximized.
    expect(canvas.querySelectorAll('.tile-pane')).toHaveLength(1);
    expect(screen.queryByTestId(`maximize-${otherLeaf.id}`)).toBeNull();

    await user.click(screen.getByTestId(`maximize-${leafId}`));
    expect(canvas.getAttribute('data-maximized')).toBe('');
    expect(canvas.querySelectorAll('.tile-pane')).toHaveLength(2);
  });

  it('resizes splits with the keyboard within clamped bounds', () => {
    let doc = openTile(emptyCanvasDoc(), { id: 't1', entityId: 'e', kind: 'chat' });
    const leafId = leafNodes(doc.root)[0]!.id;
    doc = splitLeaf(doc, leafId, 'row').doc;
    const splitId = doc.root.kind === 'split' ? doc.root.id : '';
    const ratioOf = (candidate: CanvasDoc): number =>
      candidate.root.kind === 'split' ? candidate.root.ratio : 1;

    render(<Harness initial={doc} />);
    const splitter = screen.getByTestId(`splitter-${splitId}`);

    fireEvent.keyDown(splitter, { key: 'ArrowRight' });
    expect(ratioOf(currentDoc())).toBeCloseTo(0.55, 5);

    for (let i = 0; i < 30; i += 1) {
      fireEvent.keyDown(splitter, { key: 'ArrowLeft' });
    }
    expect(ratioOf(currentDoc())).toBeCloseTo(0.1, 5);
  });
});
