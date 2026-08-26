import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { CommandPalette } from './CommandPalette';
import { useCommandRegistry } from '../commands/command-registry';
import { usePaletteStore } from '../state/palette-store';

function resetState(): void {
  useCommandRegistry.setState({ commands: {}, order: [], usage: {} });
  usePaletteStore.setState({ open: false });
  window.localStorage.clear();
}

describe('CommandPalette', () => {
  beforeEach(() => {
    resetState();
  });

  afterEach(() => {
    cleanup();
    resetState();
  });

  it('lists available commands grouped by section when opened', () => {
    const ran = vi.fn();
    const registry = useCommandRegistry.getState();
    registry.register({
      id: 'nav.home',
      title: 'Go to Home',
      section: 'Navigate',
      run: ran,
    });
    registry.register({
      id: 'host.retry',
      title: 'Retry Host connection',
      section: 'Host',
      when: () => true,
      run: ran,
    });
    registry.register({
      id: 'hidden.command',
      title: 'Hidden command',
      when: () => false,
      run: ran,
    });

    usePaletteStore.getState().openPalette();
    render(<CommandPalette />);

    expect(screen.getByText('Go to Home')).toBeTruthy();
    expect(screen.getByText('Retry Host connection')).toBeTruthy();
    expect(screen.getByText('Navigate')).toBeTruthy();
    expect(screen.queryByText('Hidden command')).toBeNull();
  });

  it('filters commands fuzzily as the user types', async () => {
    const user = userEvent.setup();
    const registry = useCommandRegistry.getState();
    registry.register({
      id: 'nav.home',
      title: 'Go to Home',
      section: 'Navigate',
      run: () => undefined,
    });
    registry.register({
      id: 'view.theme',
      title: 'Switch appearance theme',
      section: 'Shell',
      run: () => undefined,
    });

    usePaletteStore.getState().openPalette();
    render(<CommandPalette />);

    await user.type(screen.getByTestId('palette-input'), 'theme');
    expect(screen.getByText('Switch appearance theme')).toBeTruthy();
    expect(screen.queryByText('Go to Home')).toBeNull();

    await user.clear(screen.getByTestId('palette-input'));
    expect(screen.getByText('Go to Home')).toBeTruthy();
  });

  it('runs the highlighted command on Enter and records recency', async () => {
    const user = userEvent.setup();
    const ran = vi.fn();
    useCommandRegistry.getState().register({
      id: 'nav.home',
      title: 'Go to Home',
      section: 'Navigate',
      run: ran,
    });

    usePaletteStore.getState().openPalette();
    render(<CommandPalette />);

    // First result is highlighted by default.
    await user.keyboard('{Enter}');
    expect(ran).toHaveBeenCalledTimes(1);
    expect(usePaletteStore.getState().open).toBe(false);
    expect(useCommandRegistry.getState().usage['nav.home']?.count).toBe(1);
  });

  it('moves the highlight with arrow keys and runs with mouse click', async () => {
    const user = userEvent.setup();
    const ranA = vi.fn();
    const ranB = vi.fn();
    const registry = useCommandRegistry.getState();
    registry.register({ id: 'a.first', title: 'Alpha command', section: 'Group A', run: ranA });
    registry.register({ id: 'b.second', title: 'Beta command', section: 'Group B', run: ranB });

    usePaletteStore.getState().openPalette();
    render(<CommandPalette />);

    await user.type(screen.getByTestId('palette-input'), '{ArrowDown}');
    const activeOption = document.querySelector('[aria-selected="true"]');
    expect(activeOption?.getAttribute('id')).toBe(
      screen.getByText('Beta command').closest('[role="option"]')?.getAttribute('id'),
    );

    await user.click(screen.getByText('Alpha command'));
    expect(ranA).toHaveBeenCalledTimes(1);
    expect(ranB).not.toHaveBeenCalled();
    expect(usePaletteStore.getState().open).toBe(false);
  });

  it('shows an empty state for queries without matches', async () => {
    const user = userEvent.setup();
    useCommandRegistry
      .getState()
      .register({ id: 'only', title: 'Only command', run: () => undefined });

    usePaletteStore.getState().openPalette();
    render(<CommandPalette />);

    await user.type(screen.getByTestId('palette-input'), 'zzzz');
    expect(screen.getByRole('status').textContent).toContain('No matching commands');
  });
});
