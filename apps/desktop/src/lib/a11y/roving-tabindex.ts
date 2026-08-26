import type { KeyboardEvent } from 'react';

/**
 * Roving tabindex primitives (Phase 3.5). Composite widgets such as tab
 * lists keep one tab stop: the active item has `tabIndex = 0`, every other
 * item `-1`, and arrow keys move both focus and activation. This module is
 * the single source of that behavior for the header tab strip, tile canvas
 * pane tabs, and any later list/tab composite.
 */

/** tabIndex for the item at `index` given the currently active index. */
export function rovingTabIndex(index: number, activeIndex: number | null): 0 | -1 {
  return index === activeIndex ? 0 : -1;
}

export interface RoveOptions {
  /** Number of items in the composite. */
  count: number;
  /** Currently focused (or activated) index; -1 when nothing is focused. */
  current: number;
  onMove(next: number): void;
}

/**
 * Handles the WAI-ARIA arrow-key contract for a roving tabindex group.
 * Orientation decides which arrows move focus; Home/End jump to the ends.
 * Returns true when the event was consumed so callers can preventDefault.
 */
export function handleRovingKeys(
  event: KeyboardEvent<HTMLElement>,
  options: RoveOptions & { orientation?: 'horizontal' | 'vertical' },
): boolean {
  const { count, current, onMove } = options;
  const orientation = options.orientation ?? 'horizontal';
  const previous = orientation === 'horizontal' ? 'ArrowLeft' : 'ArrowUp';
  const next = orientation === 'horizontal' ? 'ArrowRight' : 'ArrowDown';
  let handled = false;
  let target: number | null = null;

  if (event.key === previous) {
    target = current <= 0 ? count - 1 : current - 1;
    handled = true;
  } else if (event.key === next) {
    target = current >= count - 1 ? 0 : current + 1;
    handled = true;
  } else if (event.key === 'Home') {
    target = 0;
    handled = true;
  } else if (event.key === 'End') {
    target = count - 1;
    handled = true;
  }

  if (target !== null && count > 0) {
    event.preventDefault();
    onMove(Math.min(count - 1, Math.max(0, target)));
  }
  return handled;
}
