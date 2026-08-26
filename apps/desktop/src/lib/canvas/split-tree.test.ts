import { describe, expect, it } from 'vitest';

import {
  MAX_RATIO,
  MIN_RATIO,
  clampRatio,
  cloneDoc,
  closeTile,
  emptyCanvasDoc,
  emptyLeaf,
  findLeaf,
  findTile,
  firstLeaf,
  focusTile,
  freshId,
  leafNodes,
  leafOfTile,
  moveTile,
  openTile,
  openTileInLeaf,
  parseCanvasDoc,
  serializeCanvasDoc,
  setMaximized,
  setRatio,
  splitLeaf,
  type CanvasDoc,
  type LeafNode,
} from './split-tree';

function tile(id: string, entityId = `entity-${id}`) {
  return { id, entityId, kind: 'chat' as const };
}

/** Builds a deep three-level split: (A | (B / C)) with the given ratios. */
function deepDoc(
  firstRatio: number,
  secondRatio: number,
): {
  doc: CanvasDoc;
  leaves: { a: LeafNode; b: LeafNode; c: LeafNode };
} {
  const leafA = emptyLeaf();
  const leafB = emptyLeaf();
  const leafC = emptyLeaf();
  const innerSplit = {
    kind: 'split' as const,
    id: 'split-inner',
    direction: 'column' as const,
    ratio: secondRatio,
    first: leafB,
    second: leafC,
  };
  const outerSplit = {
    kind: 'split' as const,
    id: 'split-outer',
    direction: 'row' as const,
    ratio: firstRatio,
    first: leafA,
    second: innerSplit,
  };
  return {
    doc: { version: 1, root: outerSplit, maximizedLeafId: null },
    leaves: { a: leafA, b: leafB, c: leafC },
  };
}

describe('canvas split tree', () => {
  it('opens tiles into leaves and focuses them', () => {
    let doc = emptyCanvasDoc();
    const rootLeafId = firstLeaf(doc.root).id;

    doc = openTileInLeaf(doc, rootLeafId, tile('t1'));
    doc = openTileInLeaf(doc, rootLeafId, tile('t2'));
    expect(findLeaf(doc.root, rootLeafId)?.tiles.map((t) => t.id)).toEqual(['t1', 't2']);
    expect(findLeaf(doc.root, rootLeafId)?.activeTileId).toBe('t2');

    doc = focusTile(doc, 't1');
    expect(leafOfTile(doc.root, 't1')?.activeTileId).toBe('t1');

    // Focusing an absent tile is a no-op, not a crash.
    const unchanged = focusTile(doc, 'missing');
    expect(unchanged).toEqual(doc);
  });

  it('splits leaves along either direction with a fresh pane', () => {
    let doc = emptyCanvasDoc();
    const rootLeafId = firstLeaf(doc.root).id;
    doc = openTileInLeaf(doc, rootLeafId, tile('t1'));

    const row = splitLeaf(doc, rootLeafId, 'row', tile('t2'));
    doc = row.doc;
    expect(leafNodes(doc.root)).toHaveLength(2);
    const outer = doc.root;
    if (outer.kind !== 'split') throw new Error('expected a split');
    expect(outer.direction).toBe('row');
    expect(outer.ratio).toBe(0.5);
    expect(findTile(doc.root, row.newTileId ?? '')?.entityId).toBe('entity-t2');

    // Splitting again nests arbitrarily deep.
    const keptLeaf = outer.first;
    const nested = splitLeaf(doc, keptLeaf.kind === 'leaf' ? keptLeaf.id : '', 'column');
    expect(leafNodes(nested.doc.root)).toHaveLength(3);
  });

  it('closes tiles and collapses empty panes without touching siblings', () => {
    const initial = deepDoc(0.4, 0.6);
    initial.leaves.a.tiles.push(tile('t-a'));
    initial.leaves.b.tiles.push(tile('t-b'));
    initial.leaves.c.tiles.push(tile('t-c'));

    // Closing B collapses the inner split; A and C remain.
    const afterB = closeTile(initial.doc, 't-b');
    expect(afterB).not.toBeNull();
    expect(leafNodes(afterB!.root)).toHaveLength(2);
    expect(findTile(afterB!.root, 't-a')).not.toBeNull();
    expect(findTile(afterB!.root, 't-c')).not.toBeNull();

    // Closing everything reduces to one empty pane; the tree stays valid.
    const afterC = closeTile(afterB!, 't-c');
    const afterA = closeTile(afterC!, 't-a');
    expect(afterA).not.toBeNull();
    const only = leafNodes(afterA!.root);
    expect(only).toHaveLength(1);
    expect(only[0]!.tiles).toEqual([]);

    // Closing an absent tile reports failure instead of mutating.
    expect(closeTile(afterA!, 'nope')).toBeNull();
  });

  it('clears maximization when the maximized pane collapses', () => {
    const initial = deepDoc(0.5, 0.5);
    initial.leaves.a.tiles.push(tile('t-a'));
    initial.leaves.b.tiles.push(tile('t-b'));
    initial.leaves.c.tiles.push(tile('t-c'));
    let doc = setMaximized(initial.doc, initial.leaves.c.id);
    expect(doc.maximizedLeafId).toBe(initial.leaves.c.id);

    doc = closeTile(doc, 't-c')!;
    expect(doc.maximizedLeafId).toBeNull();

    // Maximizing an unknown leaf is refused; null always clears.
    expect(setMaximized(doc, 'ghost').maximizedLeafId).toBeNull();
    expect(setMaximized(doc, null).maximizedLeafId).toBeNull();
  });

  it('moves tiles between panes and reorders within a pane', () => {
    const initial = deepDoc(0.5, 0.5);
    initial.leaves.a.tiles.push(tile('t-a1'));
    initial.leaves.a.tiles.push(tile('t-a2'));
    initial.leaves.c.tiles.push(tile('t-c1'));

    // Cross-pane move lands at the end of the destination by default.
    const moved = moveTile(initial.doc, 't-a1', initial.leaves.c.id);
    expect(moved).not.toBeNull();
    expect(leafOfTile(moved!.root, 't-a1')?.tiles.map((tile) => tile.id)).toEqual(['t-c1', 't-a1']);

    // Same-pane move reorders in place.
    const reordered = moveTile(moved!, 't-a1', initial.leaves.c.id, 0);
    expect(reordered).not.toBeNull();
    expect(leafOfTile(reordered!.root, 't-a1')?.tiles.map((tile) => tile.id)).toEqual([
      't-a1',
      't-c1',
    ]);

    // A named destination that collapsed (it was empty and its branch
    // pruned away) falls back to the first surviving empty pane.
    const collapse = deepDoc(0.5, 0.5);
    collapse.leaves.a.tiles.push(tile('only'));
    const intoEmpty = splitLeaf(collapse.doc, collapse.leaves.a.id, 'row').doc;
    // The fresh pane is empty; moving the only tile there empties A, so
    // the whole tree collapses to one empty pane that receives the tile.
    const freshPane = leafNodes(intoEmpty.root).find((leaf) => leaf.id !== collapse.leaves.a.id)!;
    const landed = moveTile(intoEmpty, 'only', freshPane.id);
    expect(landed).not.toBeNull();
    expect(leafOfTile(landed!.root, 'only')).not.toBeNull();

    // With no empty pane anywhere, a vanished destination is refused
    // rather than dumping the tile into unrelated content.
    const full = deepDoc(0.5, 0.5);
    full.leaves.a.tiles.push(tile('t-a'));
    full.leaves.b.tiles.push(tile('t-b'));
    full.leaves.c.tiles.push(tile('t-c'));
    expect(moveTile(full.doc, 't-a', 'leaf-never-existed')).toBeNull();

    // Moving an absent tile is refused too.
    expect(moveTile(initial.doc, 'nope', initial.leaves.c.id)).toBeNull();
  });

  it('clamps resize ratios to keep every pane usable', () => {
    expect(clampRatio(0.02)).toBe(MIN_RATIO);
    expect(clampRatio(1.4)).toBe(MAX_RATIO);
    expect(clampRatio(Number.NaN)).toBe(0.5);
    expect(clampRatio(0.72)).toBe(0.72);

    const doc = deepDoc(0.5, 0.5).doc;
    const resized = setRatio(doc, 'split-outer', 0.95);
    expect((resized.root as { ratio: number }).ratio).toBe(MAX_RATIO);
    // Unknown split ids are no-ops.
    expect(setRatio(doc, 'ghost', 0.3)).toEqual(doc);
  });

  it('serializes and restores multi-level layouts exactly', () => {
    const initial = deepDoc(0.37, 0.64);
    initial.leaves.a.tiles.push(tile('t-a'));
    initial.leaves.c.tiles.push(tile('t-c1'));
    initial.leaves.c.tiles.push(tile('t-c2'));
    let doc = initial.doc;
    doc = focusTile(doc, 't-c2');
    doc = setMaximized(doc, null);
    doc = setRatio(doc, 'split-inner', 0.71);

    const restored = parseCanvasDoc(serializeCanvasDoc(doc));
    expect(restored).toEqual(doc);
    // Pixel-exact means ratios survive untouched within the band.
    expect((restored!.root as { ratio: number }).ratio).toBe(0.37);
    expect(
      ((restored!.root as { second: { ratio: number } }).second as { ratio: number }).ratio,
    ).toBe(0.71);
    // Round-tripping the restoration is stable.
    expect(parseCanvasDoc(serializeCanvasDoc(restored!))).toEqual(restored);
  });

  it('rejects corrupt or off-model persisted documents', () => {
    expect(parseCanvasDoc(null)).toBeNull();
    expect(parseCanvasDoc('')).toBeNull();
    expect(parseCanvasDoc('not json at all')).toBeNull();
    expect(parseCanvasDoc('{"version":2,"root":{}}')).toBeNull();
    expect(parseCanvasDoc('[]')).toBeNull();

    // Unknown tile kinds fail validation instead of rendering garbage.
    expect(
      parseCanvasDoc(
        JSON.stringify({
          version: 1,
          maximizedLeafId: null,
          root: {
            kind: 'leaf',
            id: 'l1',
            activeTileId: null,
            tiles: [{ id: 'x', entityId: 'e', kind: 'spreadsheet' }],
          },
        }),
      ),
    ).toBeNull();

    // Duplicate node ids make targeting ambiguous and are rejected.
    const duplicated = JSON.stringify({
      version: 1,
      maximizedLeafId: null,
      root: {
        kind: 'split',
        id: 'same',
        direction: 'row',
        ratio: 0.5,
        first: { kind: 'leaf', id: 'same', tiles: [], activeTileId: null },
        second: { kind: 'leaf', id: 'other', tiles: [], activeTileId: null },
      },
    });
    expect(parseCanvasDoc(duplicated)).toBeNull();
  });

  it('normalizes out-of-band stored ratios on load', () => {
    const raw = JSON.stringify({
      version: 1,
      maximizedLeafId: null,
      root: {
        kind: 'split',
        id: 's1',
        direction: 'row',
        ratio: 0.001,
        first: { kind: 'leaf', id: 'l1', tiles: [], activeTileId: null },
        second: { kind: 'leaf', id: 'l2', tiles: [], activeTileId: null },
      },
    });
    const parsed = parseCanvasDoc(raw);
    expect(parsed).not.toBeNull();
    expect((parsed!.root as { ratio: number }).ratio).toBe(MIN_RATIO);
  });

  it('generates unique ids across calls', () => {
    const ids = new Set(Array.from({ length: 200 }, () => freshId('node')));
    expect(ids.size).toBe(200);
  });
  it('clones deeply so mutations never leak between versions', () => {
    const doc = deepDoc(0.5, 0.5).doc;
    const copy = cloneDoc(doc);
    if (copy.root.kind === 'split') {
      copy.root.ratio = 0.8;
    }
    expect((doc.root as { ratio: number }).ratio).toBe(0.5);
    // Opening a tile leaves the original doc untouched.
    const opened = openTile(doc, tile('t9'));
    expect(leafNodes(opened.root)[0]!.tiles).toHaveLength(1);
    expect(leafNodes(doc.root)[0]!.tiles).toHaveLength(0);
  });
});
