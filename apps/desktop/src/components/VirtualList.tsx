import {
  useCallback,
  useEffect,
  useId,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';

import { joinClassNames } from './Button';

export type VirtualRow<TItem> =
  { kind: 'header'; key: string; label: string } | { kind: 'item'; key: string; item: TItem };

export interface VirtualListHandle {
  scrollToIndex: (index: number, align?: 'start' | 'end' | 'auto') => void;
  focus: () => void;
}

export interface VirtualListProps<TItem> {
  rows: Array<VirtualRow<TItem>>;
  rowHeight: (row: VirtualRow<TItem>, index: number) => number;
  renderItem: (item: TItem, index: number, state: { active: boolean }) => ReactNode;
  renderHeader?: (label: string) => ReactNode;
  overscanCount?: number;
  /**
   * Fixed viewport height in px. When omitted the list measures its container
   * (ResizeObserver) with a sane fallback, which also keeps tests deterministic.
   */
  viewportHeight?: number;
  activeIndex?: number | null;
  onActiveIndexChange?: (index: number | null) => void;
  onActivateItem?: (item: TItem, index: number) => void;
  className?: string;
  ariaLabel?: string;
  emptyState?: ReactNode;
  /** React 19 style ref for imperative scrolling/focus. */
  ref?: React.Ref<VirtualListHandle>;
}

const FALLBACK_VIEWPORT_HEIGHT = 480;

/** Smallest row index whose extent reaches past `position`. */
function firstRowEndingAfter(offsets: Float64Array, rowCount: number, position: number): number {
  let lo = 0;
  let hi = Math.max(0, rowCount - 1);
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((offsets[mid + 1] ?? Number.POSITIVE_INFINITY) > position) {
      hi = mid;
    } else {
      lo = mid + 1;
    }
  }
  return lo;
}

/** Smallest row index that starts at or below-edge `position`; rowCount when none. */
function firstRowStartingAtOrAfter(
  offsets: Float64Array,
  rowCount: number,
  position: number,
): number {
  let lo = 0;
  let hi = rowCount;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((offsets[mid] ?? Number.POSITIVE_INFINITY) >= position) {
      hi = mid;
    } else {
      lo = mid + 1;
    }
  }
  return lo;
}

/**
 * Windowed list with variable row heights, sticky group headers, and
 * keyboard navigation. Only visible rows (+overscan) exist in the DOM.
 */
export function VirtualList<TItem>(props: VirtualListProps<TItem>): ReactNode {
  const {
    rows,
    rowHeight,
    renderItem,
    renderHeader,
    overscanCount = 8,
    viewportHeight,
    activeIndex = null,
    onActiveIndexChange,
    onActivateItem,
    className,
    ariaLabel,
    emptyState,
    ref,
  } = props;

  const containerRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [measuredHeight, setMeasuredHeight] = useState(viewportHeight ?? FALLBACK_VIEWPORT_HEIGHT);
  // Unique per instance so several lists can coexist without colliding
  // aria-activedescendant/option ids.
  const listId = useId();
  const optionId = (index: number): string => `${listId}-opt-${index}`;

  useEffect(() => {
    if (viewportHeight !== undefined) {
      setMeasuredHeight(viewportHeight);
      return;
    }
    const element = containerRef.current;
    if (!element || typeof ResizeObserver === 'undefined') {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const height = entries[0]?.contentRect.height;
      if (height && height > 0) {
        setMeasuredHeight(height);
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [viewportHeight]);

  const offsets = useMemo(() => {
    const acc = new Float64Array(rows.length + 1);
    for (let index = 0; index < rows.length; index += 1) {
      const row = rows[index];
      acc[index + 1] = (acc[index] ?? 0) + (row ? rowHeight(row, index) : 0);
    }
    return acc;
  }, [rows, rowHeight]);

  const headerEndByIndex = useMemo(() => {
    const ends = new Float64Array(rows.length).fill(Number.NaN);
    let pendingHeader = -1;
    for (let index = 0; index < rows.length; index += 1) {
      const row = rows[index];
      if (!row || row.kind !== 'header') {
        continue;
      }
      if (pendingHeader >= 0) {
        ends[pendingHeader] = offsets[index] ?? 0;
      }
      pendingHeader = index;
    }
    if (pendingHeader >= 0) {
      ends[pendingHeader] = offsets[rows.length] ?? 0;
    }
    return ends;
  }, [rows, offsets]);

  const itemPositions = useMemo(() => {
    const positions = new Map<number, number>();
    rows.forEach((row, index) => {
      if (row?.kind === 'item') {
        positions.set(index, positions.size);
      }
    });
    return positions;
  }, [rows]);

  const itemCount = itemPositions.size;
  const totalHeight = offsets[rows.length] ?? 0;

  const scrollToIndex = useCallback(
    (index: number, align: 'start' | 'end' | 'auto' = 'auto') => {
      const element = containerRef.current;
      if (!element || index < 0 || index >= rows.length) {
        return;
      }
      const top = offsets[index] ?? 0;
      const row = rows[index];
      const height = row ? rowHeight(row, index) : 0;
      let nextScrollTop = element.scrollTop;
      if (align === 'start') {
        nextScrollTop = top;
      } else if (align === 'end') {
        nextScrollTop = top + height - measuredHeight;
      } else if (top < element.scrollTop) {
        nextScrollTop = top;
      } else if (top + height > element.scrollTop + measuredHeight) {
        nextScrollTop = top + height - measuredHeight;
      }
      element.scrollTop = Math.max(0, nextScrollTop);
      setScrollTop(Math.max(0, nextScrollTop));
    },
    [offsets, rowHeight, rows, measuredHeight],
  );

  useImperativeHandle(ref, () => ({
    scrollToIndex,
    focus: () => containerRef.current?.focus(),
  }));

  const handleScroll = useCallback((event: React.UIEvent<HTMLDivElement>) => {
    setScrollTop(event.currentTarget.scrollTop);
  }, []);

  const moveActive = useCallback(
    (step: 1 | -1 | 'first' | 'last') => {
      if (!onActiveIndexChange || itemCount === 0) {
        return;
      }
      const currentPosition = activeIndex === null ? -1 : (itemPositions.get(activeIndex) ?? -1);
      let nextPosition: number;
      if (step === 'first') {
        nextPosition = 0;
      } else if (step === 'last') {
        nextPosition = itemCount - 1;
      } else if (currentPosition < 0) {
        nextPosition = step === 1 ? 0 : itemCount - 1;
      } else {
        nextPosition = Math.min(itemCount - 1, Math.max(0, currentPosition + step));
      }
      for (const [rowIndex, position] of itemPositions) {
        if (position === nextPosition) {
          onActiveIndexChange(rowIndex);
          scrollToIndex(rowIndex, 'auto');
          return;
        }
      }
    },
    [activeIndex, itemPositions, itemCount, onActiveIndexChange, scrollToIndex],
  );

  const handleKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      switch (event.key) {
        case 'ArrowDown':
          event.preventDefault();
          moveActive(1);
          break;
        case 'ArrowUp':
          event.preventDefault();
          moveActive(-1);
          break;
        case 'Home':
          event.preventDefault();
          moveActive('first');
          break;
        case 'End':
          event.preventDefault();
          moveActive('last');
          break;
        case 'PageDown':
          event.preventDefault();
          containerRef.current?.scrollBy({ top: measuredHeight });
          break;
        case 'PageUp':
          event.preventDefault();
          containerRef.current?.scrollBy({ top: -measuredHeight });
          break;
        case 'Enter':
        case ' ': {
          if (activeIndex !== null && onActivateItem) {
            const row = rows[activeIndex];
            if (row?.kind === 'item') {
              event.preventDefault();
              onActivateItem(row.item, activeIndex);
            }
          }
          break;
        }
        default:
          break;
      }
    },
    [moveActive, measuredHeight, activeIndex, onActivateItem, rows],
  );

  if (rows.length === 0) {
    return (
      <div
        ref={containerRef}
        className={joinClassNames('vlist-viewport', className)}
        style={{ height: viewportHeight === undefined ? undefined : viewportHeight }}
      >
        {emptyState ?? <p className="muted vlist-empty">Nothing to show.</p>}
      </div>
    );
  }

  const start = Math.max(0, firstRowEndingAfter(offsets, rows.length, scrollTop) - overscanCount);
  const endExclusive = Math.min(
    rows.length,
    firstRowStartingAtOrAfter(offsets, rows.length, scrollTop + measuredHeight) + overscanCount,
  );

  const rendered: ReactNode[] = [];
  for (let index = start; index < endExclusive; index += 1) {
    const row = rows[index];
    if (!row) {
      continue;
    }
    const naturalTop = offsets[index] ?? 0;
    if (row.kind === 'header') {
      const headerHeight = rowHeight(row, index);
      const boundary = headerEndByIndex[index] ?? totalHeight;
      const stickyTop = Math.max(
        naturalTop,
        Math.min(scrollTop, Math.max(naturalTop, boundary - headerHeight)),
      );
      rendered.push(
        <div
          key={row.key}
          data-virtual-row={index}
          data-virtual-header="true"
          role="presentation"
          className="vlist-header"
          style={{ position: 'absolute', top: stickyTop, left: 0, right: 0, zIndex: 2 }}
        >
          {renderHeader ? renderHeader(row.label) : <span>{row.label}</span>}
        </div>,
      );
      continue;
    }
    const isActive = index === activeIndex;
    rendered.push(
      <div
        key={row.key}
        data-virtual-row={index}
        role="option"
        id={optionId(index)}
        aria-selected={isActive}
        aria-setsize={itemCount}
        aria-posinset={(itemPositions.get(index) ?? 0) + 1}
        className={joinClassNames('vlist-row', isActive && 'vlist-row-active')}
        style={{ position: 'absolute', top: naturalTop, left: 0, right: 0 }}
        onClick={() => onActivateItem?.(row.item, index)}
      >
        {renderItem(row.item, index, { active: isActive })}
      </div>,
    );
  }

  const activeRendered =
    activeIndex !== null &&
    activeIndex >= start &&
    activeIndex < endExclusive &&
    rows[activeIndex]?.kind === 'item';

  return (
    <div
      ref={containerRef}
      className={joinClassNames('vlist-viewport', className)}
      role="listbox"
      aria-label={ariaLabel}
      tabIndex={0}
      aria-activedescendant={activeRendered ? optionId(activeIndex) : undefined}
      onKeyDown={handleKeyDown}
      onScroll={handleScroll}
    >
      <div className="vlist-canvas" style={{ height: totalHeight, position: 'relative' }}>
        {rendered}
      </div>
    </div>
  );
}

/** Builds grouped rows preserving item order, emitting a header per new label. */
export function buildGroupedRows<TItem>(
  items: TItem[],
  groupFor: (item: TItem) => string | null,
  keyFor: (item: TItem, index: number) => string,
): Array<VirtualRow<TItem>> {
  const rows: Array<VirtualRow<TItem>> = [];
  let currentLabel: string | null = null;
  items.forEach((item, index) => {
    const label = groupFor(item);
    if (label !== null && label !== currentLabel) {
      rows.push({ kind: 'header', key: `header:${label}`, label });
      currentLabel = label;
    }
    rows.push({ kind: 'item', key: keyFor(item, index), item });
  });
  return rows;
}
