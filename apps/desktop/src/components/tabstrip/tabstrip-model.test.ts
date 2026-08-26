import { describe, expect, it } from 'vitest';

import {
  OVERFLOW_RESERVE_PX,
  computeTargetIndex,
  planOverflow,
  type TabRect,
} from './tabstrip-model';

function rects(widths: number[]): TabRect[] {
  let cursor = 0;
  return widths.map((width, index) => {
    const rect = { id: `t${index}`, left: cursor, right: cursor + width };
    cursor += width;
    return rect;
  });
}

describe('drag reorder targeting', () => {
  it('stays put while the pointer remains inside the grabbed tab', () => {
    const layout = rects([80, 80, 80]);
    expect(computeTargetIndex(layout, 0, 40)).toBe(0);
    expect(computeTargetIndex(layout, 1, 120)).toBe(1);
  });

  it('swaps forward past midpoints and backward before them', () => {
    const layout = rects([100, 100, 100]);
    // Strictly beyond tab 2's midpoint -> move two slots.
    expect(computeTargetIndex(layout, 0, 251)).toBe(2);
    // Just past tab 1's midpoint -> one slot.
    expect(computeTargetIndex(layout, 0, 151)).toBe(1);
    // Exactly on a midpoint is not yet a crossing.
    expect(computeTargetIndex(layout, 0, 250)).toBe(1);
    // Backward strictly below tab 0's midpoint; the midpoint itself is
    // not yet a crossing.
    expect(computeTargetIndex(layout, 2, 49)).toBe(0);
    expect(computeTargetIndex(layout, 2, 149)).toBe(1);
    expect(computeTargetIndex(layout, 2, 150)).toBe(2);
  });

  it('tolerates unknown indices and empty layouts', () => {
    expect(computeTargetIndex([], 3, 10)).toBe(3);
    const layout = rects([50]);
    expect(computeTargetIndex(layout, 7, 10)).toBe(7);
  });
});

describe('overflow planning', () => {
  const W = 96;

  it('keeps everything inline when measurements are unavailable', () => {
    expect(planOverflow(0, [W, W, W]).inlineCount).toBe(3);
    expect(planOverflow(-10, []).inlineCount).toBe(0);
  });

  it('fits as many tabs as the container allows and overflows the rest', () => {
    // Container fits exactly three tabs plus the reserved controls.
    const available = OVERFLOW_RESERVE_PX + W * 3 + 4 * 2;
    const plan = planOverflow(available, [W, W, W, W, W]);
    expect(plan.inlineCount).toBe(3);
    expect(plan.inlineCount).toBeLessThan(5);
  });

  it('always keeps at least one tab inline', () => {
    const plan = planOverflow(OVERFLOW_RESERVE_PX + 20, [W, W, W]);
    expect(plan.inlineCount).toBe(1);
  });

  it('does not show an overflow control when every tab truly fits', () => {
    const available = W * 3 + 4 * 2;
    const plan = planOverflow(available, [W, W, W]);
    expect(plan.inlineCount).toBe(3);
  });

  it('overflows one tab when the full set just misses the space', () => {
    const available = OVERFLOW_RESERVE_PX + W * 3 + 4 * 2;
    // Four tabs need 96*4 + gaps (396) but only 380 exist.
    const plan = planOverflow(available, [W, W, W, W]);
    expect(plan.inlineCount).toBe(3);
  });
});
