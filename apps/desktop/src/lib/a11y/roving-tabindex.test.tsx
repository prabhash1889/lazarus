import { describe, expect, it, vi } from 'vitest';
import type { KeyboardEvent } from 'react';

import { handleRovingKeys, rovingTabIndex } from './roving-tabindex';

/** Minimal stand-in: handleRovingKeys only reads `key` and calls preventDefault. */
function keyEvent(key: string): KeyboardEvent<HTMLElement> {
  return {
    key,
    preventDefault: () => undefined,
  } as unknown as KeyboardEvent<HTMLElement>;
}

describe('rovingTabIndex', () => {
  it('marks only the active index as a tab stop', () => {
    expect(rovingTabIndex(0, 0)).toBe(0);
    expect(rovingTabIndex(1, 0)).toBe(-1);
    expect(rovingTabIndex(2, null)).toBe(-1);
  });
});

describe('handleRovingKeys', () => {
  it('moves right and wraps from the last item (horizontal)', () => {
    const onMove = vi.fn();
    expect(handleRovingKeys(keyEvent('ArrowRight'), { count: 3, current: 2, onMove })).toBe(true);
    expect(onMove).toHaveBeenCalledWith(0);
  });

  it('moves left and wraps from the first item (horizontal)', () => {
    const onMove = vi.fn();
    handleRovingKeys(keyEvent('ArrowLeft'), { count: 3, current: 0, onMove });
    expect(onMove).toHaveBeenCalledWith(2);
  });

  it('uses vertical arrows for vertical orientation', () => {
    const onMove = vi.fn();
    expect(
      handleRovingKeys(keyEvent('ArrowDown'), { count: 3, current: 1, onMove, orientation: 'vertical' }),
    ).toBe(true);
    expect(onMove).toHaveBeenCalledWith(2);
    expect(
      handleRovingKeys(keyEvent('ArrowRight'), { count: 3, current: 1, onMove, orientation: 'vertical' }),
    ).toBe(false);
    expect(onMove).toHaveBeenCalledTimes(1);
  });

  it('jumps to the ends with Home and End', () => {
    const onMove = vi.fn();
    handleRovingKeys(keyEvent('Home'), { count: 4, current: 2, onMove });
    handleRovingKeys(keyEvent('End'), { count: 4, current: 2, onMove });
    expect(onMove).toHaveBeenNthCalledWith(1, 0);
    expect(onMove).toHaveBeenNthCalledWith(2, 3);
  });

  it('ignores movement in an empty composite and leaves other keys alone', () => {
    const onMove = vi.fn();
    expect(handleRovingKeys(keyEvent('ArrowLeft'), { count: 0, current: 0, onMove })).toBe(true);
    expect(onMove).not.toHaveBeenCalled();
    expect(handleRovingKeys(keyEvent('a'), { count: 3, current: 0, onMove })).toBe(false);
    expect(onMove).not.toHaveBeenCalled();
  });
});
