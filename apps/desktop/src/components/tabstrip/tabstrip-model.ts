/**
 * Pure geometry helpers for the header tab strip: drag-reorder targeting
 * and overflow planning. Extracted so both are unit-testable without a
 * real layout engine.
 */

export interface TabRect {
  id: string;
  left: number;
  right: number;
}

/**
 * Which index the tab currently at `fromIndex` should occupy when the
 * pointer sits at `x`. Uses tab midpoints so the swap point feels natural;
 * returns `fromIndex` when no boundary was crossed.
 */
export function computeTargetIndex(
  rects: readonly TabRect[],
  fromIndex: number,
  x: number,
): number {
  const current = rects[fromIndex];
  if (current === undefined) {
    return fromIndex;
  }
  let target = fromIndex;
  if (x > current.right) {
    for (let i = fromIndex + 1; i < rects.length; i += 1) {
      const rect = rects[i];
      if (rect !== undefined && x > (rect.left + rect.right) / 2) {
        target = i;
      }
    }
  } else if (x < current.left) {
    for (let i = fromIndex - 1; i >= 0; i -= 1) {
      const rect = rects[i];
      if (rect !== undefined && x < (rect.left + rect.right) / 2) {
        target = i;
      }
    }
  }
  return target;
}

/** Widths reserved for the new-Epic button plus the overflow control. */
export const OVERFLOW_RESERVE_PX = 84;
const TAB_GAP_PX = 4;

export interface OverflowPlan {
  /** Number of leading tabs rendered inline; the rest collapse into the menu. */
  inlineCount: number;
}

/**
 * Plans how many tabs fit inline given the container's inner width. When
 * every tab genuinely fits they all stay inline (no control needed);
 * otherwise the space reserved for the overflow and new-tab controls is
 * subtracted first and tabs fill greedily. At least one tab always stays
 * inline so the strip never empties out.
 */
export function planOverflow(availableWidth: number, tabWidths: readonly number[]): OverflowPlan {
  const count = tabWidths.length;
  if (availableWidth <= 0 || count === 0) {
    return { inlineCount: count };
  }
  let total = 0;
  for (let i = 0; i < count; i += 1) {
    total += tabWidths[i]! + (i === 0 ? 0 : TAB_GAP_PX);
  }
  if (total <= availableWidth) {
    return { inlineCount: count };
  }
  let used = OVERFLOW_RESERVE_PX;
  let fitting = 0;
  for (const width of tabWidths) {
    const cost = width + (fitting === 0 ? 0 : TAB_GAP_PX);
    if (used + cost > availableWidth) {
      break;
    }
    used += cost;
    fitting += 1;
  }
  return { inlineCount: Math.max(1, fitting) };
}
