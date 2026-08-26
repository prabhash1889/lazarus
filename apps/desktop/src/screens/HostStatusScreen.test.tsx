import { cleanup, render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import HostStatusScreen from './HostStatusScreen';
import { useConnectionStore } from '../lib/host/connection-store';
import * as productionConnection from '../lib/host/production-connection';

function renderScreen(): { unmount: () => void } {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <HostStatusScreen />
    </QueryClientProvider>,
  );
  return { unmount: view.unmount };
}

function setState(patch: Partial<ReturnType<typeof useConnectionStore.getState>>): void {
  useConnectionStore.setState(patch);
}

describe('HostStatusScreen', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useConnectionStore.getState().reset();
  });

  afterEach(() => {
    cleanup();
  });

  it('renders the connected snapshot with methods and capabilities', () => {
    setState({
      phase: 'authenticated',
      hostVersion: '1.2.3',
      servingStatus: 'SERVING',
      startedAtUnixMs: null,
      outageId: 'outage-abc',
      liveSequence: 12,
      methods: [
        { name: 'system.getInfo', support: 'supported', version: '1.1', fallback: null },
        { name: 'task.list', support: 'supported', version: '1.2', fallback: null },
      ],
      capabilities: [{ name: 'events', enabled: true }],
    });

    renderScreen();

    const pill = screen.getByText('Connected');
    expect(pill.getAttribute('data-phase')).toBe('authenticated');
    expect(screen.getByText('1.2.3')).toBeTruthy();
    expect(screen.getByText(/seq 12/)).toBeTruthy();
    expect(screen.getByText(/system\.getInfo=1\.1/)).toBeTruthy();
    expect(screen.getByText(/task\.list=1\.2/)).toBeTruthy();
    const capabilityItem = screen.getByText('events').closest('li');
    expect(capabilityItem?.textContent).toBe('events - enabled');
  });

  it('shows the auth-failed banner with the canonical code and a Retry action', async () => {
    const retryNow = vi
      .spyOn(productionConnection.connectionManager, 'retryNow')
      .mockResolvedValue(undefined);
    setState({
      phase: 'auth-failed',
      lastErrorCode: 'UNAUTHENTICATED',
      lastErrorMessage: 'missing or invalid local token',
      reconnectAttempt: 3,
    });

    renderScreen();

    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('[UNAUTHENTICATED]');
    expect(alert.textContent).toContain('missing or invalid local token');
    expect(screen.getByText('attempt 3')).toBeTruthy();

    await userEvent.click(screen.getByRole('button', { name: 'Retry now' }));
    expect(retryNow).toHaveBeenCalledTimes(1);
  });

  it('renders degraded and disconnected phases distinctly', () => {
    setState({ phase: 'degraded', servingStatus: 'NOT_SERVING' });
    renderScreen();
    expect(screen.getByText('Degraded (not serving)').getAttribute('data-phase')).toBe('degraded');
    cleanup();

    setState({ phase: 'disconnected' });
    renderScreen();
    expect(screen.getByText('Disconnected').getAttribute('data-phase')).toBe('disconnected');
  });
});
