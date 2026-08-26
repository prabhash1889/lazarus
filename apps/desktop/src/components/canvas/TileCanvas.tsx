import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react';

import { joinClassNames } from '../Button';
import { handleRovingKeys, rovingTabIndex } from '../../lib/a11y/roving-tabindex';
import {
  clampRatio,
  closeTile,
  findLeaf,
  focusTile,
  MAX_RATIO,
  MIN_RATIO,
  moveTile,
  openTileInLeaf,
  setMaximized,
  setRatio,
  splitLeaf,
  type CanvasDoc,
  type CanvasNode,
  type LeafNode,
  type SplitDirection,
  type SplitNode,
  type TileBinding,
  type TileKind,
} from '../../lib/canvas/split-tree';

/**
 * The persisted tile canvas (Phase 3.4): renders a binary split tree with
 * resizable panes, stacked tile tabs per pane, split/move/close/maximize
 * operations, and an empty-canvas state. The component is pure view +
 * intent: every mutation flows through `onChange` with the next immutable
 * doc so the owner can persist it (per-Task Host record).
 */

const TILE_MIME = 'application/x-lazarus-tile-id';
const KEYBOARD_RESIZE_STEP = 0.05;

export interface TileCanvasProps {
  doc: CanvasDoc;
  onChange(next: CanvasDoc): void;
  /** Renders one tile's content; called only for the active tile. */
  renderTile(binding: TileBinding): ReactNode;
  /** Creates a tile binding for the empty-canvas / split affordances. */
  createTile(kind: TileKind): TileBinding;
}

export function TileCanvas(props: TileCanvasProps): ReactNode {
  const { doc } = props;
  const maximized = doc.maximizedLeafId !== null ? findLeaf(doc.root, doc.maximizedLeafId) : null;
  const visibleRoot: CanvasNode = maximized ?? doc.root;
  return (
    <div className="tile-canvas" data-testid="tile-canvas" data-maximized={maximized?.id ?? ''}>
      <NodeView node={visibleRoot} {...props} depth={0} />
    </div>
  );
}

interface NodeViewProps extends TileCanvasProps {
  node: CanvasNode;
  depth: number;
}

function NodeView({ node, ...canvas }: NodeViewProps): ReactNode {
  return node.kind === 'leaf' ? (
    <LeafView leaf={node} {...canvas} />
  ) : (
    <SplitView split={node} {...canvas} />
  );
}

interface SplitViewProps extends Omit<NodeViewProps, 'node'> {
  split: SplitNode;
}

function SplitView({ split, doc, onChange, ...rest }: SplitViewProps): ReactNode {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [dragging, setDragging] = useState(false);

  const resizeWith = useCallback(
    (fractionWithinContainer: number) => {
      onChange(setRatio(doc, split.id, fractionWithinContainer));
    },
    [doc, onChange, split.id],
  );

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>): void => {
    event.preventDefault();
    const container = containerRef.current;
    if (container === null || event.button !== 0) {
      return;
    }
    const rect = container.getBoundingClientRect();
    const horizontal = split.direction === 'row';
    setDragging(true);

    const handleMove = (moveEvent: PointerEvent): void => {
      const fraction = horizontal
        ? (moveEvent.clientX - rect.left) / rect.width
        : (moveEvent.clientY - rect.top) / rect.height;
      resizeWith(fraction);
    };
    const finish = () => {
      setDragging(false);
      window.removeEventListener('pointermove', handleMove);
      window.removeEventListener('pointerup', finish);
    };
    window.addEventListener('pointermove', handleMove);
    window.addEventListener('pointerup', finish);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const decrease = split.direction === 'row' ? 'ArrowLeft' : 'ArrowUp';
    const increase = split.direction === 'row' ? 'ArrowRight' : 'ArrowDown';
    if (event.key !== decrease && event.key !== increase) {
      return;
    }
    event.preventDefault();
    const delta = event.key === increase ? KEYBOARD_RESIZE_STEP : -KEYBOARD_RESIZE_STEP;
    resizeWith(clampRatio(split.ratio + delta));
  };

  const basis = `${(split.ratio * 100).toFixed(2)}%`;
  return (
    <div
      ref={containerRef}
      className={joinClassNames('tile-split', dragging && 'tile-split-dragging')}
      style={{ flexDirection: split.direction === 'row' ? 'row' : 'column' }}
      data-direction={split.direction}
    >
      <div className="tile-split-child" style={{ flexBasis: basis }}>
        <NodeView node={split.first} doc={doc} onChange={onChange} {...rest} />
      </div>
      <div
        role="separator"
        aria-orientation={split.direction === 'row' ? 'vertical' : 'horizontal'}
        aria-label={`Resize ${split.direction} split`}
        aria-valuenow={Math.round(clampRatio(split.ratio) * 100)}
        aria-valuemin={Math.round(MIN_RATIO * 100)}
        aria-valuemax={Math.round(MAX_RATIO * 100)}
        tabIndex={0}
        className={joinClassNames(
          'tile-splitter',
          split.direction === 'row' ? 'tile-splitter-row' : 'tile-splitter-column',
          dragging && 'tile-splitter-active',
        )}
        data-testid={`splitter-${split.id}`}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
      />
      <div className="tile-split-child" style={{ flexBasis: basis, flexGrow: 1 }}>
        <NodeView node={split.second} doc={doc} onChange={onChange} {...rest} />
      </div>
    </div>
  );
}

interface LeafViewProps extends Omit<NodeViewProps, 'node'> {
  leaf: LeafNode;
}

function LeafView({ leaf, doc, onChange, renderTile, createTile }: LeafViewProps): ReactNode {
  const maximizedHere = doc.maximizedLeafId === leaf.id;
  const anyMaximized = doc.maximizedLeafId !== null;
  // Roving tabindex for the pane's tile tabs: one tab stop per pane.
  const [focusIndex, setFocusIndex] = useState(0);
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    const index = leaf.tiles.findIndex((tile) => tile.id === leaf.activeTileId);
    if (index >= 0) {
      setFocusIndex(index);
    }
  }, [leaf.activeTileId, leaf.tiles]);

  const onTablistKeyDown = (event: KeyboardEvent<HTMLElement>): void => {
    handleRovingKeys(event, {
      count: leaf.tiles.length,
      current: focusIndex,
      orientation: 'horizontal',
      onMove: (next) => {
        setFocusIndex(next);
        tabRefs.current[next]?.focus();
      },
    });
  };

  const openIntoSplit = (direction: SplitDirection) => () =>
    onChange(splitLeaf(doc, leaf.id, direction).doc);

  const onDrop = (event: DragEvent<HTMLElement>): void => {
    const tileId = event.dataTransfer.getData(TILE_MIME);
    if (tileId === '') {
      return;
    }
    event.preventDefault();
    const moved = moveTile(doc, tileId, leaf.id);
    if (moved !== null) {
      onChange(moved);
    }
  };

  const allowDrop = (event: DragEvent<HTMLElement>): void => {
    if (event.dataTransfer.types.includes(TILE_MIME)) {
      event.preventDefault();
    }
  };

  return (
    <section
      className="tile-pane"
      data-testid={`pane-${leaf.id}`}
      data-active-tile={leaf.activeTileId ?? ''}
      onDragOver={allowDrop}
      onDrop={onDrop}
    >
      <header className="tile-pane-header">
        <span
          className="tile-pane-tablist"
          role="tablist"
          aria-label="Open tiles"
          aria-owns={leaf.tiles.map((tile) => `pane-tab-${tile.id}`).join(' ') || undefined}
        />
        <div className="tile-pane-tabs" onKeyDown={onTablistKeyDown}>
          {leaf.tiles.map((tile, index) => (
            <div key={tile.id} className="tile-tab-wrap" data-testid={`tile-${tile.id}`}>
              <button
                ref={(el) => {
                  tabRefs.current[index] = el;
                }}
                id={`pane-tab-${tile.id}`}
                type="button"
                role="tab"
                aria-selected={tile.id === leaf.activeTileId}
                tabIndex={rovingTabIndex(index, focusIndex)}
                className={joinClassNames(
                  'tile-tab',
                  tile.id === leaf.activeTileId && 'tile-tab-active',
                )}
                data-testid={`tile-tab-${tile.id}`}
                draggable
                onFocus={() => setFocusIndex(index)}
                onDragStart={(event) => {
                  event.dataTransfer.setData(TILE_MIME, tile.id);
                  event.dataTransfer.effectAllowed = 'move';
                }}
                onClick={() => onChange(focusTile(doc, tile.id))}
              >
                <span className="tile-tab-title">{tile.kind}</span>
              </button>
              <button
                type="button"
                aria-label={`Close ${tile.kind} tile`}
                className="tile-tab-close"
                data-testid={`close-${tile.id}`}
                onClick={(event) => {
                  event.stopPropagation();
                  const next = closeTile(doc, tile.id);
                  if (next !== null) {
                    onChange(next);
                  }
                }}
              >
                ×
              </button>
            </div>
          ))}
        </div>
        <div className="tile-pane-actions">
          <button
            type="button"
            className="tile-action"
            title="Split pane vertically (side by side)"
            aria-label="Split pane side by side"
            disabled={anyMaximized && !maximizedHere}
            data-testid={`split-row-${leaf.id}`}
            onClick={openIntoSplit('row')}
          >
            ▯
          </button>
          <button
            type="button"
            className="tile-action"
            title="Split pane horizontally (stacked)"
            aria-label="Split pane stacked"
            disabled={anyMaximized && !maximizedHere}
            data-testid={`split-column-${leaf.id}`}
            onClick={openIntoSplit('column')}
          >
            ▤
          </button>
          <button
            type="button"
            className="tile-action"
            title={maximizedHere ? 'Restore pane' : 'Maximize pane'}
            aria-label={maximizedHere ? `Restore pane` : `Maximize pane`}
            data-testid={`maximize-${leaf.id}`}
            onClick={() => onChange(setMaximized(doc, maximizedHere ? null : leaf.id))}
          >
            {maximizedHere ? '❐' : '□'}
          </button>
          <button
            type="button"
            className="tile-action"
            title="Close active tile"
            aria-label="Close active tile"
            disabled={leaf.activeTileId === null}
            data-testid={`close-active-${leaf.id}`}
            onClick={() => {
              if (leaf.activeTileId === null) {
                return;
              }
              const next = closeTile(doc, leaf.activeTileId);
              if (next !== null) {
                onChange(next);
              }
            }}
          >
            ✕
          </button>
        </div>
      </header>
      <div className="tile-pane-body">
        {leaf.activeTileId !== null && leaf.tiles.length > 0 ? (
          (() => {
            const active = leaf.tiles.find((tile) => tile.id === leaf.activeTileId);
            return active !== undefined ? renderTile(active) : null;
          })()
        ) : (
          <PaneEmptyState
            onCreate={(kind) => {
              onChange(openTileInLeaf(doc, leaf.id, createTile(kind)));
            }}
            canSplit={!anyMaximized || maximizedHere}
            onSplitRow={openIntoSplit('row')}
            onSplitColumn={openIntoSplit('column')}
          />
        )}
      </div>
    </section>
  );
}

const TILE_KIND_LABELS: Record<TileKind, string> = {
  chat: 'Chat',
  terminal: 'Terminal agent',
  artifact: 'Artifact',
};

interface PaneEmptyStateProps {
  onCreate(kind: TileKind): void;
  canSplit: boolean;
  onSplitRow(): void;
  onSplitColumn(): void;
}

function PaneEmptyState({
  onCreate,
  canSplit,
  onSplitRow,
  onSplitColumn,
}: PaneEmptyStateProps): ReactNode {
  return (
    <div className="tile-empty" data-testid="tile-empty-state">
      <p className="tile-empty-title">This Epic has no open tiles</p>
      <p className="muted">Open a tile or split this pane to arrange your workspace.</p>
      <div className="actions">
        {(Object.keys(TILE_KIND_LABELS) as TileKind[]).map((kind) => (
          <button
            key={kind}
            type="button"
            className="btn btn-ghost"
            data-testid={`open-${kind}`}
            onClick={() => onCreate(kind)}
          >
            Open {TILE_KIND_LABELS[kind]}
          </button>
        ))}
      </div>
      {canSplit ? (
        <div className="actions">
          <button type="button" className="link-button" onClick={onSplitRow}>
            Split side by side
          </button>
          <button type="button" className="link-button" onClick={onSplitColumn}>
            Split stacked
          </button>
        </div>
      ) : null}
    </div>
  );
}
