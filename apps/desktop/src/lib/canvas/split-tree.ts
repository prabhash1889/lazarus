import { z } from 'zod';

/**
 * The tile-canvas layout model (Phase 3.4): a binary split tree serialized
 * as JSON, supporting arbitrary nesting depth.
 *
 * Every interior node splits its area in one direction (`row` places the
 * first child on the left, `column` on top) and stores the first child's
 * fraction of the shared extent as `ratio`. Leaves hold an ordered stack of
 * tile bindings with one active tile; several leaves may bind tiles to the
 * same durable entity, and closing a tile never touches the entity itself.
 */

export type SplitDirection = 'row' | 'column';

/** Kinds a canvas tile can render. Real feature tiles replace these
 * placeholders from Phase 4 onward without changing the layout model. */
export type TileKind = 'chat' | 'terminal' | 'artifact';

export interface TileBinding {
  /** Durable identity of this tile instance; independent of its entity. */
  id: string;
  /** The durable entity this tile renders; closing the tile keeps it. */
  entityId: string;
  kind: TileKind;
}

export interface LeafNode {
  kind: 'leaf';
  id: string;
  tiles: TileBinding[];
  activeTileId: string | null;
}

export interface SplitNode {
  kind: 'split';
  id: string;
  direction: SplitDirection;
  /** Fraction of the extent given to `first`; clamped to [MIN, MAX]. */
  ratio: number;
  first: CanvasNode;
  second: CanvasNode;
}

export type CanvasNode = LeafNode | SplitNode;

export interface CanvasDoc {
  version: 1;
  root: CanvasNode;
  /** When set, that leaf fills the whole canvas until restored. */
  maximizedLeafId: string | null;
}

/** Ratio bounds keep every pane usable no matter how deep the tree nests. */
export const MIN_RATIO = 0.1;
export const MAX_RATIO = 0.9;

const TileBindingSchema = z.object({
  id: z.string().min(1),
  entityId: z.string().min(1),
  kind: z.enum(['chat', 'terminal', 'artifact']),
});

const CanvasNodeSchema: z.ZodType<CanvasNode> = z.lazy(() =>
  z.discriminatedUnion('kind', [
    z.object({
      kind: z.literal('leaf'),
      id: z.string().min(1),
      tiles: z.array(TileBindingSchema),
      activeTileId: z.string().min(1).nullable(),
    }),
    z.object({
      kind: z.literal('split'),
      id: z.string().min(1),
      direction: z.enum(['row', 'column']),
      ratio: z.number().finite(),
      first: CanvasNodeSchema,
      second: CanvasNodeSchema,
    }),
  ]),
);

export const CanvasDocSchema = z.object({
  version: z.literal(1),
  root: CanvasNodeSchema,
  maximizedLeafId: z.string().min(1).nullable(),
});

let idCounter = 0;

/** Generates a unique node or tile id. Stable across serialization. */
export function freshId(prefix: string): string {
  idCounter += 1;
  const random =
    typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? crypto.randomUUID().slice(0, 8)
      : Math.random().toString(36).slice(2, 10);
  return `${prefix}-${random}-${idCounter.toString(36)}`;
}

export function emptyLeaf(): LeafNode {
  return { kind: 'leaf', id: freshId('leaf'), tiles: [], activeTileId: null };
}

export function emptyCanvasDoc(): CanvasDoc {
  return { version: 1, root: emptyLeaf(), maximizedLeafId: null };
}

function isSplit(node: CanvasNode): node is SplitNode {
  return node.kind === 'split';
}

function isLeaf(node: CanvasNode): node is LeafNode {
  return node.kind === 'leaf';
}

/** Depth-first visitation over every node. */
export function walkNodes(node: CanvasNode, visit: (node: CanvasNode) => void): void {
  visit(node);
  if (isSplit(node)) {
    walkNodes(node.first, visit);
    walkNodes(node.second, visit);
  }
}

export function leafNodes(root: CanvasNode): LeafNode[] {
  const leaves: LeafNode[] = [];
  walkNodes(root, (node) => {
    if (isLeaf(node)) {
      leaves.push(node);
    }
  });
  return leaves;
}

export function splitNodes(root: CanvasNode): SplitNode[] {
  const splits: SplitNode[] = [];
  walkNodes(root, (node) => {
    if (isSplit(node)) {
      splits.push(node);
    }
  });
  return splits;
}

export function findLeaf(root: CanvasNode, leafId: string): LeafNode | null {
  for (const leaf of leafNodes(root)) {
    if (leaf.id === leafId) {
      return leaf;
    }
  }
  return null;
}

export function findTile(node: CanvasNode, tileId: string): TileBinding | null {
  for (const leaf of leafNodes(node)) {
    const tile = leaf.tiles.find((candidate) => candidate.id === tileId);
    if (tile !== undefined) {
      return tile;
    }
  }
  return null;
}

export function leafOfTile(node: CanvasNode, tileId: string): LeafNode | null {
  for (const leaf of leafNodes(node)) {
    if (leaf.tiles.some((tile) => tile.id === tileId)) {
      return leaf;
    }
  }
  return null;
}

/**
 * Structural clone preserving node identities; operations mutate their copy
 * and return it so React state updates stay referentially detectable.
 */
export function cloneDoc(doc: CanvasDoc): CanvasDoc {
  return JSON.parse(JSON.stringify(doc)) as CanvasDoc;
}

/** Clamps a ratio into the usable band with a small tolerance. */
export function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) {
    return 0.5;
  }
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
}

/**
 * Removes a tile from whichever leaf holds it. A leaf left empty collapses:
 * its parent split is replaced by the sibling subtree, repeatedly, so the
 * tree only ever contains panes the user can still see. Returns the updated
 * doc, or `null` when the tile did not exist.
 */
export function closeTile(doc: CanvasDoc, tileId: string): CanvasDoc | null {
  const next = cloneDoc(doc);
  const target = leafOfTile(next.root, tileId);
  if (target === null) {
    return null;
  }

  function prune(node: CanvasNode): CanvasNode {
    if (isLeaf(node)) {
      // Empty leaves collapse away unless the tree is a single pane.
      return node;
    }
    const first = prune(node.first);
    const second = prune(node.second);
    const firstEmpty = isLeaf(first) && first.tiles.length === 0;
    const secondEmpty = isLeaf(second) && second.tiles.length === 0;
    if (firstEmpty && secondEmpty) {
      return emptyLeaf();
    }
    if (firstEmpty) {
      return second;
    }
    if (secondEmpty) {
      return first;
    }
    node.first = first;
    node.second = second;
    return node;
  }

  target.tiles = target.tiles.filter((tile) => tile.id !== tileId);
  if (target.activeTileId === tileId) {
    target.activeTileId = target.tiles.at(-1)?.id ?? null;
  }
  next.root = prune(next.root);
  if (next.maximizedLeafId !== null && findLeaf(next.root, next.maximizedLeafId) === null) {
    next.maximizedLeafId = null;
  }
  return next;
}

/** Focuses a tile within its leaf. Returns the doc unchanged when missing. */
export function focusTile(doc: CanvasDoc, tileId: string): CanvasDoc {
  const next = cloneDoc(doc);
  const leaf = leafOfTile(next.root, tileId);
  if (leaf === null) {
    return next;
  }
  leaf.activeTileId = tileId;
  return next;
}

/** Opens a tile into a specific leaf, making it active. */
export function openTileInLeaf(doc: CanvasDoc, leafId: string, tile: TileBinding): CanvasDoc {
  const next = cloneDoc(doc);
  const leaf = findLeaf(next.root, leafId) ?? firstLeaf(next.root);
  leaf.tiles.push(tile);
  leaf.activeTileId = tile.id;
  return next;
}

export function firstLeaf(root: CanvasNode): LeafNode {
  let node = root;
  while (isSplit(node)) {
    node = node.first;
  }
  return node;
}

/** Opens a tile into the first leaf; convenience for empty canvases. */
export function openTile(doc: CanvasDoc, tile: TileBinding): CanvasDoc {
  const root = cloneDoc(doc).root;
  return openTileInLeaf({ ...doc, root }, firstLeaf(root).id, tile);
}

/**
 * Splits a leaf along `direction`, keeping the existing tiles in one half
 * and optionally placing a new tile in the other. Returns the updated doc,
 * plus the ids of both resulting leaves and the new tile when created.
 */
export function splitLeaf(
  doc: CanvasDoc,
  leafId: string,
  direction: SplitDirection,
  newTile?: TileBinding,
): { doc: CanvasDoc; keptLeafId: string; newLeafId: string; newTileId: string | null } {
  const next = cloneDoc(doc);
  const target = findLeaf(next.root, leafId) ?? firstLeaf(next.root);

  const keptLeaf: LeafNode = target.tiles.length > 0 ? target : emptyLeaf();
  if (keptLeaf !== target && target.tiles.length === 0) {
    // Replacing an empty target wholesale: keep its identity on the kept
    // side so callers holding the old id stay valid.
    keptLeaf.id = target.id;
  }
  const createdLeaf = emptyLeaf();
  let newTileId: string | null = null;
  if (newTile !== undefined) {
    createdLeaf.tiles.push(newTile);
    createdLeaf.activeTileId = newTile.id;
    newTileId = newTile.id;
  }

  const split: SplitNode = {
    kind: 'split',
    id: freshId('split'),
    direction,
    ratio: 0.5,
    first: keptLeaf,
    second: createdLeaf,
  };

  function replace(node: CanvasNode): CanvasNode {
    if (isLeaf(node) && node.id === target.id) {
      return split;
    }
    if (isSplit(node)) {
      node.first = replace(node.first);
      node.second = replace(node.second);
    }
    return node;
  }
  next.root = replace(next.root);
  return { doc: next, keptLeafId: keptLeaf.id, newLeafId: createdLeaf.id, newTileId };
}

/**
 * Moves a tile into another leaf (or reorders it within its own leaf).
 * Removal empties collapse exactly like closeTile, except when the emptied
 * leaf is itself the destination's neighborhood - the move then lands in
 * the surviving structure. Returns the updated doc, or `null` when either
 * the tile or the destination vanished mid-operation.
 */
export function moveTile(
  doc: CanvasDoc,
  tileId: string,
  targetLeafId: string,
  index?: number,
): CanvasDoc | null {
  const tile = findTile(doc.root, tileId);
  if (tile === null) {
    return null;
  }
  const sameLeaf = leafOfTile(doc.root, tileId)?.id === targetLeafId;

  const removed = closeTile(doc, tileId);
  if (removed === null) {
    return null;
  }

  const next = cloneDoc(removed);
  let destination = findLeaf(next.root, targetLeafId);
  if (destination === null) {
    if (!sameLeaf) {
      // The removal collapsed the destination subtree; refuse rather than
      // guess where the user wanted the tile.
      return null;
    }
    // Same-leaf reorder whose leaf collapsed cannot happen (it had >=2
    // tiles), but guard anyway by reopening into the first leaf.
    destination = firstLeaf(next.root);
  }
  const insertAt = Math.max(
    0,
    Math.min(index ?? destination.tiles.length, destination.tiles.length),
  );
  destination.tiles.splice(insertAt, 0, tile);
  destination.activeTileId = tile.id;
  return next;
}

/** Sets the split ratio of one interior node, clamped. No-op when absent. */
export function setRatio(doc: CanvasDoc, splitId: string, ratio: number): CanvasDoc {
  const next = cloneDoc(doc);
  const split = splitNodes(next.root).find((node) => node.id === splitId);
  if (split !== undefined) {
    split.ratio = clampRatio(ratio);
  }
  return next;
}

/** Maximizes one leaf, or clears maximization with `null`. */
export function setMaximized(doc: CanvasDoc, leafId: string | null): CanvasDoc {
  const next = cloneDoc(doc);
  next.maximizedLeafId = leafId !== null && findLeaf(next.root, leafId) !== null ? leafId : null;
  return next;
}

/** Serializes a doc for Host persistence. */
export function serializeCanvasDoc(doc: CanvasDoc): string {
  return JSON.stringify(doc);
}

/**
 * Parses a persisted document. Anything malformed - truncated JSON, wrong
 * shape, unknown version - returns `null` so callers fall back to a default
 * canvas instead of crashing on a corrupt record.
 */
export function parseCanvasDoc(raw: string | null | undefined): CanvasDoc | null {
  if (raw === null || raw === undefined || raw === '') {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  const decoded = CanvasDocSchema.safeParse(parsed);
  if (!decoded.success) {
    return null;
  }
  const doc = decoded.data;
  // Reject docs whose ids are not unique: they would make maximize/split
  // targeting ambiguous.
  const seen = new Set<string>();
  let unique = true;
  walkNodes(doc.root, (node) => {
    if (seen.has(node.id)) {
      unique = false;
    }
    seen.add(node.id);
  });
  if (!unique) {
    return null;
  }
  if (doc.maximizedLeafId !== null && !seen.has(doc.maximizedLeafId)) {
    doc.maximizedLeafId = null;
  }
  return normalizeRatios(doc);
}

function normalizeRatios(doc: CanvasDoc): CanvasDoc {
  const normalized = cloneDoc(doc);
  for (const split of splitNodes(normalized.root)) {
    split.ratio = clampRatio(split.ratio);
  }
  return normalized;
}
