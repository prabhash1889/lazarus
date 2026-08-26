import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider, createMemoryHistory } from '@tanstack/react-router';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { createAppRouter } from './router';
import { useCommandRegistry } from '../commands/command-registry';
import { engineResetForTests } from '../commands/CommandHost';
import { useConnectionStore } from '../lib/host/connection-store';
import { resetEpicsForTests } from '../state/epics-store';
import { resetShellForTests } from '../state/shell-store';
import { usePaletteStore } from '../state/palette-store';
import { ThemeProvider } from '../theme/ThemeProvider';

function makeQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

function renderApp(): ReturnType<typeof createAppRouter> {
  const router = createAppRouter(createMemoryHistory({ initialEntries: ['/'] }));
  render(
    <ThemeProvider>
      <QueryClientProvider client={makeQueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  return router;
}

async function renderAppAndWaitForShell(): Promise<ReturnType<typeof createAppRouter>> {
  const router = renderApp();
  await screen.findByTestId('home-connection');
  return router;
}

describe('navigation reachability', () => {
  beforeEach(() => {
    useCommandRegistry.setState({ commands: {}, order: [], usage: {} });
    usePaletteStore.setState({ open: false });
    useConnectionStore.getState().reset();
    resetShellForTests();
    resetEpicsForTests();
    window.localStorage.clear();
    engineResetForTests();
  });

  afterEach(() => {
    cleanup();
  });

  it('renders the Home placeholders and connection summary', async () => {
    await renderAppAndWaitForShell();

    expect(screen.getByText('Quick actions')).toBeTruthy();
    expect(screen.getByTestId('workspaces-empty').textContent).toContain('Phase 4');
    expect(screen.getByTestId('tasks-empty').textContent).toContain('Phase 8');
    expect(screen.getByTestId('home-connection').textContent).toContain('Disconnected');
  });

  it('reaches Settings via mouse click on the header nav', async () => {
    const user = userEvent.setup();
    await renderAppAndWaitForShell();

    await user.click((await screen.findAllByText('Settings'))[0]!);
    expect(await screen.findByTestId('settings-panel')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Providers' })).toBeTruthy();
  });

  it('reaches every destination through the palette', async () => {
    const user = userEvent.setup();
    await renderAppAndWaitForShell();

    // Open the palette with Ctrl+K.
    await user.keyboard('{Control>}k{/Control}');
    expect(usePaletteStore.getState().open).toBe(true);

    await user.type(screen.getByTestId('palette-input'), 'diagnostics');
    const option = screen
      .getByText('Settings: Diagnostics')
      .closest('[role="option"]') as HTMLElement;
    expect(option.getAttribute('aria-selected')).toBe('true');
    await user.keyboard('{Enter}');

    expect(await screen.findByRole('heading', { name: 'Diagnostics' })).toBeTruthy();
    expect(usePaletteStore.getState().open).toBe(false);
  });

  it('reaches Home, Settings, and Host status via g-chords alone', async () => {
    const user = userEvent.setup();
    const router = await renderAppAndWaitForShell();

    await user.keyboard('gs');
    expect(await screen.findByTestId('settings-panel')).toBeTruthy();
    expect(router.state.location.pathname).toBe('/settings');

    await user.keyboard('gm');
    expect(router.state.location.pathname).toBe('/host-status');
    expect(await screen.findByText(/Connected|Disconnected|Connecting/)).toBeTruthy();

    await user.keyboard('gh');
    await screen.findByTestId('home-connection');
    expect(router.state.location.pathname).toBe('/');

    // A stale chord stroke must not fire after an unmatched key resets it.
    await user.keyboard('gx');
    await user.keyboard('h');
    expect(router.state.location.pathname).toBe('/');
  });

  it('opens the keybindings cheat sheet via its shortcut and lists bindings', async () => {
    const user = userEvent.setup();
    await renderAppAndWaitForShell();

    await user.keyboard('{Control>}/{/Control}');
    expect(await screen.findByRole('heading', { name: 'Keybindings' })).toBeTruthy();
    expect(document.querySelector('.keybindings-table')).not.toBeNull();
    expect(screen.getByText('Open command palette')).toBeTruthy();
  });

  it('renders the persistent tab strip with the pinned system tabs', async () => {
    await renderAppAndWaitForShell();

    expect(screen.getByTestId('tab-strip')).toBeTruthy();
    for (const pinned of ['tab-draft', 'tab-history', 'tab-settings']) {
      expect(screen.getByTestId(pinned)).toBeTruthy();
    }
  });

  it('reaches Draft and History through their tabs', async () => {
    const user = userEvent.setup();
    await renderAppAndWaitForShell();

    await user.click(screen.getByTestId('tab-draft'));
    expect(await screen.findByTestId('draft-placeholder')).toBeTruthy();

    await user.click(screen.getByTestId('tab-history'));
    expect(await screen.findByTestId('history-placeholder')).toBeTruthy();
  });

  it('opens an Epic tab from a deep link and registers it in the strip', async () => {
    const router = await renderAppAndWaitForShell();

    await router.navigate({ to: '/epic/$taskId', params: { taskId: 'epic-deep-link' } });
    expect(await screen.findByTestId('epic-placeholder')).toBeTruthy();
    expect(screen.getByTestId('tab-epic-deep-link')).toBeTruthy();
    expect(router.state.location.pathname).toBe('/epic/epic-deep-link');
  });

  it('creates stub Epic tabs via the palette New Epic command', async () => {
    const user = userEvent.setup();
    const router = await renderAppAndWaitForShell();

    await user.keyboard('{Control>}k{/Control}');
    await user.type(screen.getByTestId('palette-input'), 'new epic');
    await user.keyboard('{Enter}');

    expect(await screen.findByTestId('epic-placeholder')).toBeTruthy();
    expect(router.state.location.pathname).toMatch(/^\/epic\/epic-/);
    // The canvas starts empty inside the Epic tab (wired fully in the
    // screens commit).
  });
});
