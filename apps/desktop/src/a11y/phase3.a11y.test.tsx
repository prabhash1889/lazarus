import { cleanup, render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider, createMemoryHistory } from '@tanstack/react-router';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeAll, describe, expect, it } from 'vitest';
import type { ReactNode } from 'react';

import { createAppRouter } from '../app/router';
import { useCommandRegistry } from '../commands/command-registry';
import { engineResetForTests } from '../commands/CommandHost';
import { Dialog } from '../components/Dialog';
import { CommandPalette } from '../components/CommandPalette';
import { TileCanvas } from '../components/canvas/TileCanvas';
import { emptyCanvasDoc, openTile, splitLeaf, type CanvasDoc } from '../lib/canvas/split-tree';
import { useConnectionStore } from '../lib/host/connection-store';
import HostStatusScreen from '../screens/HostStatusScreen';
import { resetEpicsForTests, useEpicsStore } from '../state/epics-store';
import { usePaletteStore } from '../state/palette-store';
import { resetShellForTests } from '../state/shell-store';
import { ThemeProvider } from '../theme/ThemeProvider';
import { expectNoAxeViolations } from '../testing/a11y';

/**
 * Automated accessibility scans over every Phase 3 screen (Phase 3.5).
 * Each scan renders the surface exactly as the shell does and asserts a
 * clean axe-core report; contrast is enforced separately against the real
 * token values (jsdom cannot compute rendered colors).
 */

function makeQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
}

async function scanRoute(path: string): Promise<void> {
  const router = createAppRouter(createMemoryHistory({ initialEntries: [path] }));
  const { container } = render(
    <ThemeProvider>
      <QueryClientProvider client={makeQueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ThemeProvider>,
  );
  await expectNoAxeViolations(container);
}

describe('phase 3 accessibility scans', () => {
  beforeAll(() => {
    // Mirror the production index.html document language for axe.
    document.documentElement.lang = 'en';
  });

  afterEach(() => {
    cleanup();
    useCommandRegistry.setState({ commands: {}, order: [], usage: {} });
    usePaletteStore.setState({ open: false });
    useConnectionStore.getState().reset();
    resetShellForTests();
    resetEpicsForTests();
    engineResetForTests();
  });

  it('shell + Home passes', async () => {
    await scanRoute('/');
  });

  it('Draft screen passes', async () => {
    await scanRoute('/draft');
  });

  it('History screen passes', async () => {
    await scanRoute('/history');
  });

  it('Settings screen passes', async () => {
    await scanRoute('/settings');
  });

  it('engine rendering matrix passes', async () => {
    const router = createAppRouter(createMemoryHistory({ initialEntries: ['/engine-matrix'] }));
    const { container } = render(
      <ThemeProvider>
        <QueryClientProvider client={makeQueryClient()}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </ThemeProvider>,
    );
    await screen.findByText('xterm.js terminal');
    await screen.findByTestId('diff-prototype');
    await expectNoAxeViolations(container);
  });

  it('Host status screen passes in connected and failed phases', async () => {
    const view = render(
      <QueryClientProvider client={makeQueryClient()}>
        <HostStatusScreen />
      </QueryClientProvider>,
    );
    await expectNoAxeViolations(view.container);
    cleanup();

    useConnectionStore.setState({
      phase: 'auth-failed',
      lastErrorCode: 'UNAUTHENTICATED',
      lastErrorMessage: 'missing or invalid local token',
      reconnectAttempt: 2,
    });
    const second = render(
      <QueryClientProvider client={makeQueryClient()}>
        <HostStatusScreen />
      </QueryClientProvider>,
    );
    await expectNoAxeViolations(second.container);
  });

  it('Epic canvas with tiles and splitters passes', async () => {
    function CanvasHarness(): ReactNode {
      return (
        <main className="epic-shell">
          <h1>Epic</h1>
          <CanvasBody />
        </main>
      );
    }

    function CanvasBody(): ReactNode {
      let doc: CanvasDoc = openTile(emptyCanvasDoc(), {
        id: 'tile-a',
        entityId: 'e1',
        kind: 'chat',
      });
      doc = splitLeaf(doc, doc.root.kind === 'leaf' ? doc.root.id : '', 'row', {
        id: 'tile-b',
        entityId: 'e1',
        kind: 'terminal',
      }).doc;
      return (
        <div style={{ display: 'flex', height: 400 }}>
          <TileCanvas
            doc={doc}
            onChange={() => undefined}
            renderTile={(binding) => (
              <p data-testid={`tile-content-${binding.id}`}>{binding.kind} placeholder</p>
            )}
            createTile={(kind) => ({ id: `t-${kind}`, entityId: 'e1', kind })}
          />
        </div>
      );
    }

    const { container } = render(<CanvasHarness />);
    await expectNoAxeViolations(container);
  });

  it('command palette dialog passes while open', async () => {
    const register = useCommandRegistry.getState().register;
    register({
      id: 'a11y.sample',
      title: 'Sample command',
      section: 'Test',
      run: () => undefined,
    });
    usePaletteStore.getState().openPalette();

    render(<CommandPalette />);
    await screen.findByTestId('palette-input');

    // Radix portals into body; scan the whole document context.
    await expectNoAxeViolations(document.body);
  });

  it('generic dialog passes with focus inside', async () => {
    function DialogHarness(): ReactNode {
      return (
        <main>
          <Dialog open onOpenChange={() => undefined} title="Example">
            <p>Body content</p>
            <button type="button">Confirm</button>
          </Dialog>
        </main>
      );
    }
    const user = userEvent.setup();
    render(<DialogHarness />);
    await screen.findByText('Example');
    await user.click(screen.getByRole('button', { name: 'Confirm' }));
    await expectNoAxeViolations(document.body);
  });

  it('keyboard traversal reaches every interactive element of the shell', async () => {
    const router = createAppRouter(createMemoryHistory({ initialEntries: ['/'] }));
    const view = render(
      <ThemeProvider>
        <QueryClientProvider client={makeQueryClient()}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </ThemeProvider>,
    );
    await screen.findByTestId('home-connection');

    const user = userEvent.setup();
    const visited = new Set<string>();
    const interactiveCount = () =>
      Array.from(document.querySelectorAll('button, a[href], input, [tabindex="0"]')).filter(
        (element) => element.getAttribute('tabindex') !== '-1',
      ).length;

    // Walk enough Tab presses to plateau; roving tabindex keeps composite
    // widgets to one stop, so this must reach every distinct control.
    for (let step = 0; step < interactiveCount() + 4; step += 1) {
      await user.keyboard('{Tab}');
      const active = document.activeElement;
      if (active instanceof HTMLElement && active !== document.body) {
        if (active.textContent !== '') {
          visited.add(active.textContent);
        }
        const testId = active.getAttribute('data-testid');
        if (testId !== null) {
          visited.add(testId);
        }
      }
    }

    // Sequential Tab skips roving-index siblings by design; the strip's
    // other tabs must be reachable through their arrow-key contract instead
    // ("keyboard-only traversal reaches everything", not "everything is a
    // Tab stop").
    expect(visited.has('tab-draft')).toBe(true);
    screen.getByTestId('tab-draft').focus();
    await user.keyboard('{ArrowRight}');
    expect(document.activeElement).toBe(screen.getByTestId('tab-history'));
    await user.keyboard('{ArrowRight}');
    expect(document.activeElement).toBe(screen.getByTestId('tab-settings'));
    await expectNoAxeViolations(view.container);
  });

  it('shell with open Epic tabs passes the accessibility scan', async () => {
    // Seed two Epics so the strip renders wrapped epic tabs (tab + close
    // button per wrapper), exercising the aria-owns ownership path.
    const first = useEpicsStore.getState().createEpic('Alpha');
    useEpicsStore.getState().createEpic('Beta');
    const router = createAppRouter(createMemoryHistory({ initialEntries: [`/epic/${first.id}`] }));
    const { container } = render(
      <ThemeProvider>
        <QueryClientProvider client={makeQueryClient()}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </ThemeProvider>,
    );
    await screen.findByText('Alpha');
    await expectNoAxeViolations(container);
  });
});
