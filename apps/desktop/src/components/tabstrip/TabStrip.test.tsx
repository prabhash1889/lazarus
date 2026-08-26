import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { resetEpicsForTests, useEpicsStore } from '../../state/epics-store';
import { resetShellForTests, useShellStore } from '../../state/shell-store';
import { TabStrip } from './TabStrip';

function setupStrip(): {
  select: ReturnType<typeof vi.fn>;
  close: ReturnType<typeof vi.fn>;
  reorder: ReturnType<typeof vi.fn>;
  newEpic: ReturnType<typeof vi.fn>;
} {
  const handlers = {
    select: vi.fn(),
    close: vi.fn(),
    reorder: vi.fn(),
    newEpic: vi.fn(),
  };
  render(
    <TabStrip
      activeTab="draft"
      onSelect={handlers.select}
      onClose={handlers.close}
      onReorder={handlers.reorder}
      onNewEpic={handlers.newEpic}
    />,
  );
  return handlers;
}

function seedEpics(count: number): string[] {
  const ids: string[] = [];
  for (let i = 0; i < count; i += 1) {
    const entity = useEpicsStore.getState().createEpic(`Epic ${i + 1}`);
    ids.push(entity.id);
    useShellStore.setState((state) => ({ epicTabs: [...state.epicTabs, entity.id] }));
  }
  return ids;
}

function expectFocused(element: Element): void {
  expect(document.activeElement).toBe(element);
}

describe('header tab strip', () => {
  afterEach(() => {
    cleanup();
    resetShellForTests();
    resetEpicsForTests();
    vi.restoreAllMocks();
  });

  it('renders pinned tabs plus one tab per open Epic', () => {
    seedEpics(2);
    setupStrip();
    expect(screen.getByTestId('tab-draft')).toBeTruthy();
    expect(screen.getByTestId('tab-history')).toBeTruthy();
    expect(screen.getByTestId('tab-settings')).toBeTruthy();
    expect(screen.getByText('Epic 1')).toBeTruthy();
    expect(screen.getByText('Epic 2')).toBeTruthy();
    expect(screen.getByTestId('new-epic')).toBeTruthy();
    expect(screen.queryByTestId('tab-overflow')).toBeNull();
  });

  it('selects tabs through clicks', async () => {
    const handlers = setupStrip();
    await userEvent.setup().click(screen.getByTestId('tab-history'));
    expect(handlers.select).toHaveBeenCalledWith('history');
  });

  it('closes Epic tabs via the affordance without touching pinned tabs', async () => {
    const [id] = seedEpics(1);
    const handlers = setupStrip();
    await userEvent.setup().click(screen.getByTestId(`close-tab-${id}`));
    expect(handlers.close).toHaveBeenCalledWith(id);
    // Pinned tabs have no close affordance at all.
    expect(screen.queryByTestId(/close-tab-(draft|history|settings)/)).toBeNull();
  });

  it('closes Epic tabs on middle-click', () => {
    const [id] = seedEpics(1);
    const handlers = setupStrip();
    const tabLabel = screen.getByRole('tab', { name: 'Epic 1' });
    fireEvent.mouseDown(tabLabel, { button: 1 });
    expect(handlers.close).toHaveBeenCalledWith(id);
  });

  it('closes the focused Epic tab with the Delete key', () => {
    const [id] = seedEpics(1);
    const handlers = setupStrip();
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Epic 1' }), { key: 'Delete' });
    expect(handlers.close).toHaveBeenCalledWith(id);
  });

  it('reorders Epics by dragging past neighboring midpoints', () => {
    const ids = seedEpics(3);
    const handlers = setupStrip();

    const tabs = ids.map((id) => screen.getByTestId(`tab-${id}`));
    // Fake geometry: three 100px tabs starting at x=0.
    tabs.forEach((el, index) => {
      el.getBoundingClientRect = () =>
        ({
          left: index * 100,
          right: index * 100 + 100,
          top: 0,
          bottom: 30,
          width: 100,
          height: 30,
          x: index * 100,
          y: 0,
          toJSON: () => undefined,
        }) as DOMRect;
    });

    const firstLabel = screen.getByRole('tab', { name: 'Epic 1' });
    fireEvent.pointerDown(firstLabel, { button: 0, clientX: 50 });
    fireEvent.pointerMove(firstLabel, { clientX: 251 });
    fireEvent.pointerUp(firstLabel);

    expect(handlers.reorder).toHaveBeenCalledWith(0, 2);
  });

  it('ignores tiny pointer jiggles as clicks, not drags', () => {
    seedEpics(2);
    const handlers = setupStrip();
    const label = screen.getByRole('tab', { name: 'Epic 1' });
    fireEvent.pointerDown(label, { button: 0, clientX: 50 });
    fireEvent.pointerMove(label, { clientX: 52 });
    fireEvent.pointerUp(label);
    expect(handlers.reorder).not.toHaveBeenCalled();
  });

  it('collapses into an overflow menu with 20+ open Epics and still reaches every tab', async () => {
    const ids = seedEpics(24);
    const handlers = setupStrip();

    // Simulate a narrow strip where only two 96px tabs fit next to the
    // reserved controls; every other tab reports its true width too.
    const container = screen.getByTestId('tab-strip');
    container.getBoundingClientRect = () =>
      ({
        left: 0,
        right: 280,
        top: 0,
        bottom: 32,
        width: 280,
        height: 32,
        x: 0,
        y: 0,
        toJSON: () => undefined,
      }) as DOMRect;
    for (const id of ids) {
      const el = screen.getByTestId(`tab-${id}`);
      el.getBoundingClientRect = () =>
        ({
          left: 0,
          right: 96,
          top: 0,
          bottom: 32,
          width: 96,
          height: 32,
          x: 0,
          y: 0,
          toJSON: () => undefined,
        }) as DOMRect;
    }

    // Re-render measurement by toggling the store (dep of the layout effect).
    useShellStore.setState((state) => ({ epicTabs: [...state.epicTabs] }));

    const overflow = await screen.findByTestId('tab-overflow');
    expect(overflow.textContent?.trim()).toBe('+22');

    await userEvent.setup().click(overflow);
    const lastHidden = screen.getByTestId(`overflow-item-${ids[23]}`);
    await userEvent.setup().click(lastHidden);
    expect(handlers.select).toHaveBeenCalledWith(ids[23]);
  });

  it('creates Epics through the new-tab button', async () => {
    const handlers = setupStrip();
    await userEvent.setup().click(screen.getByTestId('new-epic'));
    expect(handlers.newEpic).toHaveBeenCalledTimes(1);
  });

  describe('roving tabindex keyboard navigation', () => {
    it('keeps a single tab stop on the active tab', () => {
      seedEpics(2);
      render(
        <TabStrip
          activeTab="history"
          onSelect={vi.fn()}
          onClose={vi.fn()}
          onReorder={vi.fn()}
          onNewEpic={vi.fn()}
        />,
      );
      const tablist = screen.getByTestId('tab-list');
      expect(tablist.getAttribute('role')).toBe('tablist');
      const tabs = screen.getAllByRole('tab');
      expect(tabs.map((tab) => tab.getAttribute('tabindex'))).toEqual(['-1', '0', '-1', '-1', '-1']);
    });

    it('moves the roving tab stop with arrow keys without activating', async () => {
      const handlers = setupStrip();
      const user = userEvent.setup();
      const first = screen.getByTestId('tab-draft');
      first.focus();
      await user.keyboard('{ArrowRight}');
      expectFocused(screen.getByTestId('tab-history'));
      // No seeded Epics: the last tab in the list is Settings.
      await user.keyboard('{End}');
      expectFocused(screen.getByTestId('tab-settings'));
      expect(handlers.select).not.toHaveBeenCalled();

      await user.keyboard('{Home}');
      expectFocused(screen.getByTestId('tab-draft'));
    });

    it('wraps from the last tab back to the first', async () => {
      setupStrip();
      const user = userEvent.setup();
      screen.getByTestId('tab-settings').focus();
      await user.keyboard('{ArrowRight}');
      expectFocused(screen.getByTestId('tab-draft'));
    });

    it('follows selection when the active tab changes externally', () => {
      const handlers = { select: vi.fn(), close: vi.fn(), reorder: vi.fn(), newEpic: vi.fn() };
      const [id] = seedEpics(1);
      const { rerender } = render(
        <TabStrip
          activeTab="draft"
          onSelect={handlers.select}
          onClose={handlers.close}
          onReorder={handlers.reorder}
          onNewEpic={handlers.newEpic}
        />,
      );
      rerender(
        <TabStrip
          activeTab={id!}
          onSelect={handlers.select}
          onClose={handlers.close}
          onReorder={handlers.reorder}
          onNewEpic={handlers.newEpic}
        />,
      );
      const tabs = screen.getAllByRole('tab');
      expect(tabs[tabs.length - 1]?.getAttribute('tabindex')).toBe('0');
      expect(tabs[0]?.getAttribute('tabindex')).toBe('-1');
    });
  });

  describe('overflow menu focus management', () => {
    function renderNarrowStrip(ids: string[]): void {
      render(
        <TabStrip
          activeTab="draft"
          onSelect={vi.fn()}
          onClose={vi.fn()}
          onReorder={vi.fn()}
          onNewEpic={vi.fn()}
        />,
      );
      const container = screen.getByTestId('tab-strip');
      container.getBoundingClientRect = () =>
        ({
          left: 0,
          right: 280,
          top: 0,
          bottom: 32,
          width: 280,
          height: 32,
          x: 0,
          y: 0,
          toJSON: () => undefined,
        }) as DOMRect;
      for (const id of ids) {
        const el = screen.getByTestId(`tab-${id}`);
        el.getBoundingClientRect = () =>
          ({
            left: 0,
            right: 96,
            top: 0,
            bottom: 32,
            width: 96,
            height: 32,
            x: 0,
            y: 0,
            toJSON: () => undefined,
          }) as DOMRect;
      }
      useShellStore.setState((state) => ({ epicTabs: [...state.epicTabs] }));
    }

    it('focuses the first item on open, contains Tab, and restores on Escape', async () => {
      const ids = seedEpics(4);
      const user = userEvent.setup();
      renderNarrowStrip(ids);

      await user.click(await screen.findByTestId('tab-overflow'));
      expect(document.activeElement).toBe(screen.getByTestId(`overflow-item-${ids[2]}`));

      // Tab cycles within the menu instead of leaving it.
      await user.keyboard('{Tab}');
      expect(screen.getByTestId('tab-strip').contains(document.activeElement)).toBe(true);
      expect(document.activeElement?.getAttribute('role')).toBe('menuitem');

      await user.keyboard('{Escape}');
      expect(screen.queryByTestId('tab-overflow')?.getAttribute('aria-expanded')).toBe('false');
      expect(document.activeElement).toBe(await screen.findByTestId('tab-overflow'));
    });
  });
});
