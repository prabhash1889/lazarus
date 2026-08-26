import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { VirtualList, type VirtualListHandle, type VirtualRow } from './VirtualList';

function itemRows(count: number): Array<VirtualRow<number>> {
  return Array.from({ length: count }, (_, index) => ({
    kind: 'item' as const,
    key: `item-${index}`,
    item: index,
  }));
}

afterEach(() => {
  cleanup();
});

describe('VirtualList', () => {
  it('keeps DOM node count bounded while scrolling a synthetic 50k-row list', () => {
    const listRef = { current: null as VirtualListHandle | null };
    render(
      <VirtualList
        ref={listRef}
        rows={itemRows(50_000)}
        rowHeight={() => 32}
        viewportHeight={600}
        overscanCount={8}
        renderItem={(item) => <span>Item {item}</span>}
      />,
    );

    const countRows = () => document.querySelectorAll('[data-virtual-row]').length;
    expect(countRows()).toBeLessThanOrEqual(600 / 32 + 16 + 2);

    act(() => {
      listRef.current?.scrollToIndex(49_999, 'end');
    });
    const bottomRows = countRows();
    expect(bottomRows).toBeLessThanOrEqual(600 / 32 + 16 + 2);
    expect(screen.getByText('Item 49999')).toBeTruthy();

    const container = document.querySelector('.vlist-viewport') as HTMLElement;
    container.scrollTop = 25_000 * 32;
    fireEvent.scroll(container);
    expect(countRows()).toBeLessThanOrEqual(600 / 32 + 16 + 2);
    expect(screen.getByText('Item 25000')).toBeTruthy();
    // The full 50k dataset never materializes in the DOM.
    expect(countRows()).toBeLessThan(100);
  });

  it('positions variable-height rows at exact offsets', () => {
    const rows: Array<VirtualRow<string>> = [
      { kind: 'item', key: 'a', item: 'alpha' },
      { kind: 'item', key: 'b', item: 'beta' },
      { kind: 'item', key: 'c', item: 'gamma' },
    ];
    const heights = [40, 80, 30];
    render(
      <VirtualList
        rows={rows}
        rowHeight={(_row, index) => heights[index] ?? 32}
        viewportHeight={200}
        overscanCount={0}
        renderItem={(item) => <span>{item}</span>}
      />,
    );

    const alpha = document.querySelector('[data-virtual-row="0"]') as HTMLElement;
    const beta = document.querySelector('[data-virtual-row="1"]') as HTMLElement;
    const gamma = document.querySelector('[data-virtual-row="2"]') as HTMLElement;
    expect(alpha.style.top).toBe('0px');
    expect(beta.style.top).toBe('40px');
    expect(gamma.style.top).toBe('120px');

    const container = document.querySelector('.vlist-viewport') as HTMLElement;
    container.scrollTop = 90;
    fireEvent.scroll(container);
    // Row covering scrollTop=90 is beta [40,120); gamma starts at 120 < 90+200.
    expect(document.querySelector('[data-virtual-row="0"]')).toBeNull();
    expect(document.querySelector('[data-virtual-row="1"]')).not.toBeNull();
    expect(document.querySelector('[data-virtual-row="2"]')).not.toBeNull();
  });

  it('clamps sticky headers to the viewport top within their group range', () => {
    const rows: Array<VirtualRow<number>> = [
      { kind: 'header', key: 'h1', label: 'Group One' },
      ...Array.from({ length: 20 }, (_, index) => ({
        kind: 'item' as const,
        key: `g1-${index}`,
        item: index,
      })),
      { kind: 'header', key: 'h2', label: 'Group Two' },
      ...Array.from({ length: 20 }, (_, index) => ({
        kind: 'item' as const,
        key: `g2-${index}`,
        item: 100 + index,
      })),
    ];

    render(
      <VirtualList
        rows={rows}
        rowHeight={(row) => (row.kind === 'header' ? 28 : 32)}
        viewportHeight={300}
        renderItem={(item) => <span>Row {item}</span>}
        renderHeader={(label) => <span>{label}</span>}
      />,
    );

    const headerOne = () => document.querySelector('[data-virtual-header]') as HTMLElement | null;
    // Natural position before scrolling.
    expect(headerOne()?.style.top).toBe('0px');

    const container = document.querySelector('.vlist-viewport') as HTMLElement;
    container.scrollTop = 200;
    fireEvent.scroll(container);
    // Canvas coordinates: stuck at the viewport top equals scrollTop.
    expect(headerOne()?.style.top).toBe('200px');

    container.scrollTop = 700; // Inside group two's range (starts at 28+20*32=668).
    fireEvent.scroll(container);
    // Group one's header is outside the render window; group two now sticks.
    expect(document.querySelector('[data-virtual-row="0"]')).toBeNull();
    const headers = Array.from(document.querySelectorAll('[data-virtual-header]')) as HTMLElement[];
    const groupTwoHeader = headers.find((element) => element.textContent === 'Group Two');
    expect(groupTwoHeader?.style.top).toBe('700px');
  });

  it('navigates with the keyboard and activates with Enter', async () => {
    const user = userEvent.setup();
    const onActivate = vi.fn();

    function Harness(): ReactNode {
      const [active, setActive] = useState<number | null>(null);
      return (
        <VirtualList
          rows={itemRows(10)}
          rowHeight={() => 32}
          viewportHeight={160}
          activeIndex={active}
          onActiveIndexChange={setActive}
          onActivateItem={onActivate}
          ariaLabel="Numbers"
          renderItem={(item) => <span>Item {item}</span>}
        />
      );
    }
    render(<Harness />);

    const viewport = screen.getByRole('listbox', { name: 'Numbers' });
    await user.click(viewport);
    expect(document.activeElement).toBe(viewport);

    await user.keyboard('{ArrowDown}');
    expect(viewport.getAttribute('aria-activedescendant')).toBe('vlist-opt-0');
    await user.keyboard('{ArrowDown}{ArrowDown}');
    expect(viewport.getAttribute('aria-activedescendant')).toBe('vlist-opt-2');
    expect(
      screen.getByText('Item 2').closest('[role="option"]')?.getAttribute('aria-selected'),
    ).toBe('true');

    await user.keyboard('{End}');
    expect(viewport.getAttribute('aria-activedescendant')).toBe('vlist-opt-9');
    await user.keyboard('{Home}');
    expect(viewport.getAttribute('aria-activedescendant')).toBe('vlist-opt-0');

    await user.keyboard('{ArrowDown}');
    await user.keyboard('{Enter}');
    expect(onActivate).toHaveBeenCalledWith(1, 1);
  });

  it('activates items with a mouse click', async () => {
    const user = userEvent.setup();
    const onActivate = vi.fn();
    render(
      <VirtualList
        rows={itemRows(5)}
        rowHeight={() => 32}
        viewportHeight={160}
        onActivateItem={onActivate}
        renderItem={(item) => <span>Item {item}</span>}
      />,
    );

    await user.click(screen.getByText('Item 3'));
    expect(onActivate).toHaveBeenCalledWith(3, 3);
  });

  it('renders the empty state when there are no rows', () => {
    render(
      <VirtualList<string>
        rows={[]}
        rowHeight={() => 32}
        viewportHeight={120}
        emptyState={<p>No commands found</p>}
        renderItem={(item) => <span>{item}</span>}
      />,
    );
    expect(screen.getByText('No commands found')).toBeTruthy();
    expect(document.querySelector('[role="listbox"]')).toBeNull();
  });
});
