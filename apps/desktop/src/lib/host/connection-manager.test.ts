import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { pushToast, useToastStore } from '../../state/toast-store';
import type { HostProbe } from '../protocol/client';
import {
  ConnectionManager,
  MAX_BACKOFF_MS,
  nextBackoffMs,
  type ConnectionDeps,
} from './connection-manager';
import { applyEventFrame, useConnectionStore } from './connection-store';

function probe(overrides?: Partial<HostProbe>): HostProbe {
  return {
    hostVersion: '0.1.0',
    capabilities: [{ name: 'events', enabled: true }],
    startedAtUnixMs: 1756100000000,
    servingStatus: 'SERVING',
    methods: [
      { name: 'system.getInfo', support: 'supported', version: '1.1', fallback: null },
      { name: 'system.health', support: 'supported', version: '1.0', fallback: null },
    ],
    ...overrides,
  };
}

interface Harness {
  manager: ConnectionManager;
  probes: HostProbe[];
  openEventsCalls: Array<string | null>;
  emitFrame: (frame: unknown) => void;
  emitClosed: (closed: { reason: string; detail?: string | null }) => void;
  pendingTimers: Array<{ at: number; fn: () => void }>;
  runTimers(): Promise<void>;
  failNextProbeWith: (error: unknown) => void;
}

function harness(initialProbes: HostProbe[]): Harness {
  const probes = [...initialProbes];
  const failures: unknown[] = [];
  let failureIndex = 0;
  const openEventsCalls: Array<string | null> = [];
  let frameHandler: ((frame: unknown) => void) | null = null;
  let closedHandler: ((closed: { reason: string; detail?: string | null }) => void) | null = null;
  const pendingTimers: Array<{ at: number; fn: () => void }> = [];

  const deps: ConnectionDeps = {
    client: {
      async probeHost() {
        if (failureIndex < failures.length) {
          throw failures[failureIndex++];
        }
        const next = probes.shift();
        if (next === undefined) {
          throw Object.assign(new Error('no probe configured'), {
            code: 'UNAVAILABLE',
          });
        }
        return next;
      },
    },
    async openEvents(lastOutageId) {
      openEventsCalls.push(lastOutageId);
    },
    onEventFrame(handler) {
      frameHandler = handler;
      return () => {
        frameHandler = null;
      };
    },
    onEventsClosed(handler) {
      closedHandler = handler;
      return () => {
        closedHandler = null;
      };
    },
    scheduleDelay(_ms, fn) {
      pendingTimers.push({ at: _ms, fn });
      return () => {};
    },
  };

  const manager = new ConnectionManager(deps);
  return {
    manager,
    probes,
    openEventsCalls,
    emitFrame: (frame) => frameHandler?.(frame),
    emitClosed: (closed) => closedHandler?.(closed),
    pendingTimers,
    async runTimers(maxRounds = 3) {
      // Bounded rounds: a perpetually failing Host keeps scheduling new
      // attempts forever by design, so draining unconditionally would hang.
      for (let round = 0; round < maxRounds && pendingTimers.length > 0; round++) {
        const batch = pendingTimers.splice(0);
        for (const timer of batch) {
          timer.fn();
          // Let microtasks (async connect cycles) settle.
          await Promise.resolve();
          await Promise.resolve();
        }
      }
    },
    failNextProbeWith(error: unknown) {
      failures.push(error);
    },
  };
}

const typedFailure = (code: string): Error => Object.assign(new Error(`failure ${code}`), { code });

describe('nextBackoffMs', () => {
  it('doubles with jitter and never exceeds the cap', () => {
    for (let attempt = 0; attempt < 10; attempt++) {
      const delay = nextBackoffMs(attempt, () => 0.5);
      expect(delay).toBeLessThanOrEqual(MAX_BACKOFF_MS);
    }
    expect(nextBackoffMs(0, () => 0.5)).toBe(Math.round(500 * 1));
    expect(nextBackoffMs(0, () => 0)).toBe(Math.round(500 * 0.8));
    expect(nextBackoffMs(0, () => 1)).toBe(Math.round(500 * 1.2));
    // The ceiling applies before jitter.
    expect(nextBackoffMs(50, () => 0)).toBe(Math.round(MAX_BACKOFF_MS * 0.8));
  });
});

describe('ConnectionManager', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useToastStore.setState({ toasts: [] });
    useConnectionStore.getState().reset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('connects once and lands in authenticated with live data wiring', async () => {
    const h = harness([probe()]);
    await h.manager.start();

    expect(useConnectionStore.getState().phase).toBe('authenticated');
    expect(useConnectionStore.getState().hostVersion).toBe('0.1.0');
    expect(h.openEventsCalls).toEqual([null]);

    // Frames flow into the shared store.
    h.emitFrame({ type: 'outage', outageId: 'outage-1' });
    h.emitFrame({
      type: 'snapshot',
      workspaces: [{ id: 'w1', name: 'Repo' }],
      tasks: [],
    });
    h.emitFrame({ type: 'live', sequence: 3 });
    expect(useConnectionStore.getState().outageId).toBe('outage-1');
    expect(useConnectionStore.getState().workspaces).toEqual([{ id: 'w1', name: 'Repo' }]);
    expect(useConnectionStore.getState().liveSequence).toBe(3);

    h.manager.stop();
  });

  it('marks degraded when the host answers but is not serving', async () => {
    const h = harness([probe({ servingStatus: 'NOT_SERVING' })]);
    await h.manager.start();
    expect(useConnectionStore.getState().phase).toBe('degraded');
    h.manager.stop();
  });

  it('surfaces auth failures as a distinct phase with a clear error', async () => {
    const h = harness([]);
    h.failNextProbeWith(typedFailure('UNAUTHENTICATED'));
    await h.manager.start();

    const state = useConnectionStore.getState();
    expect(state.phase).toBe('auth-failed');
    expect(state.lastErrorCode).toBe('UNAUTHENTICATED');
    expect(
      useToastStore
        .getState()
        .toasts.some((toast) => toast.title.includes('rejected the local token')),
    ).toBe(true);
    h.manager.stop();
  });

  it('reconnects automatically after a stream loss and resubscribes', async () => {
    const h = harness([probe(), probe({ hostVersion: '0.2.0' })]);
    await h.manager.start();
    expect(useConnectionStore.getState().phase).toBe('authenticated');

    h.emitClosed({ reason: 'completed' });
    expect(useConnectionStore.getState().phase).toBe('reconnecting');
    expect(useToastStore.getState().toasts.some((t) => t.title === 'Host connection lost')).toBe(
      true,
    );

    await h.runTimers();

    expect(useConnectionStore.getState().phase).toBe('authenticated');
    expect(useConnectionStore.getState().hostVersion).toBe('0.2.0');
    // Resubscribed exactly once more, still without a known outage id (no
    // tombstone was ever applied in this scenario).
    expect(h.openEventsCalls).toEqual([null, null]);
    expect(useToastStore.getState().toasts.some((toast) => toast.kind === 'info')).toBe(true);
    h.manager.stop();
  });

  it('keeps typed errors immediate while disconnected - no silent hangs', async () => {
    const h = harness([]);
    h.failNextProbeWith(typedFailure('DEADLINE_EXCEEDED'));
    await h.manager.start();
    expect(useConnectionStore.getState().lastErrorCode).toBe('DEADLINE_EXCEEDED');
    expect(useConnectionStore.getState().reconnectAttempt).toBe(1);

    // Repeated failures keep climbing attempts with capped backoff.
    h.failNextProbeWith(typedFailure('UNAVAILABLE'));
    await h.runTimers();
    expect(useConnectionStore.getState().reconnectAttempt).toBeGreaterThanOrEqual(2);
    h.manager.stop();
  });

  it('ignores duplicate outage tombstones for the same incarnation', () => {
    applyEventFrame({ type: 'outage', outageId: 'outage-7' });
    const firstState = useConnectionStore.getState();
    expect(firstState.outageId).toBe('outage-7');

    const toastCountBefore = useToastStore.getState().toasts.length;
    applyEventFrame({ type: 'outage', outageId: 'outage-7' });
    expect(useToastStore.getState().toasts.length).toBe(toastCountBefore);
  });

  it('announces a genuine restart when a new outage id arrives', () => {
    applyEventFrame({ type: 'outage', outageId: 'outage-a' });
    applyEventFrame({ type: 'snapshot', workspaces: [], tasks: [] });

    pushToast({ kind: 'info', title: 'sentinel' });
    const before = useToastStore.getState().toasts.length;

    applyEventFrame({ type: 'outage', outageId: 'outage-b' });
    expect(useConnectionStore.getState().outageId).toBe('outage-b');
    const toasts = useToastStore.getState().toasts;
    expect(toasts.length).toBe(before + 1);
    expect(toasts.at(-1)?.title).toContain('Host restarted');
  });

  it('repairs state wholesale from an authoritative snapshot', () => {
    applyEventFrame({
      type: 'snapshot',
      workspaces: [
        { id: 'a', name: 'First' },
        { id: 'b', name: 'Second' },
      ],
      tasks: [{ id: 't1', workspaceId: 'a', title: 'Fix login' }],
    });
    const state = useConnectionStore.getState();
    expect(state.workspaces).toHaveLength(2);
    expect(state.tasks[0]?.title).toBe('Fix login');

    // A later snapshot replaces everything rather than merging.
    applyEventFrame({ type: 'snapshot', workspaces: [{ id: 'c', name: 'Third' }], tasks: [] });
    expect(useConnectionStore.getState().workspaces).toEqual([{ id: 'c', name: 'Third' }]);
    expect(useConnectionStore.getState().tasks).toEqual([]);
  });

  it('drops off-contract event frames instead of corrupting state', () => {
    applyEventFrame({ type: 'live', sequence: -5 });
    applyEventFrame({ nonsense: true });
    applyEventFrame({ type: 'snapshot', workspaces: 'not-an-array', tasks: [] });
    expect(useConnectionStore.getState().workspaces).toEqual([]);
    expect(useConnectionStore.getState().liveSequence).toBeNull();
  });

  it('stop() disconnects cleanly and swallows later closures', async () => {
    const h = harness([probe()]);
    await h.manager.start();
    h.manager.stop();
    expect(useConnectionStore.getState().phase).toBe('disconnected');

    h.emitClosed({ reason: 'completed' });
    expect(useConnectionStore.getState().phase).toBe('disconnected');
  });
});
