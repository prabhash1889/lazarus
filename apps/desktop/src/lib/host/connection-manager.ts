import { pushToast } from '../../state/toast-store';
import type { HostProbe } from '../protocol/client';

import { applyEventFrame, useConnectionStore, type ConnectionPhase } from './connection-store';

/**
 * Capped exponential backoff with jitter for reconnection attempts:
 * 500ms base, doubling up to a 15s ceiling, each delay jittered within
 * +-20% so many clients never reconnect in lockstep.
 */
export const BASE_BACKOFF_MS = 500;
export const MAX_BACKOFF_MS = 15_000;

export function nextBackoffMs(attempt: number, random: () => number = Math.random): number {
  const clamped = Math.max(0, attempt);
  const doubling = BASE_BACKOFF_MS * 2 ** Math.min(clamped, 8);
  const capped = Math.min(MAX_BACKOFF_MS, doubling);
  const jitter = 0.8 + 0.4 * random();
  return Math.round(capped * jitter);
}

/** What the connection manager needs from its transport. */
export interface ConnectionDeps {
  /** Executes contract-decoded unary probes (see `LazarusProtocolClient`). */
  client: {
    probeHost(options?: { signal?: AbortSignal }): Promise<HostProbe>;
  };
  /**
   * Opens the event stream. Failures inside the pump surface later through
   * `onEventsClosed`; a synchronous rejection here can only mean the pump
   * is already running, which callers may ignore.
   */
  openEvents(lastOutageId: string | null): Promise<void>;
  /** Global frame feed; returns an unsubscribe function. */
  onEventFrame(handler: (frame: unknown) => void): () => void;
  /** Stream-ended feed; returns an unsubscribe function. */
  onEventsClosed(handler: (closed: { reason: string; detail?: string | null }) => void): () => void;
  /**
   * Schedules `fn` after `ms`, returning its canceller. Injectable so tests
   * drive time deterministically.
   */
  scheduleDelay(ms: number, fn: () => void): () => void;
}

const HEALTHY_PHASES: ReadonlySet<ConnectionPhase> = new Set(['authenticated', 'degraded']);

/**
 * Owns the Desktop's Host connection lifecycle: connect at startup, surface
 * every transition in the shared store, resubscribe the event stream after
 * any drop using capped exponential backoff with jitter, and repair missed
 * deltas from authoritative snapshots (restart tombstones dedupe by outage
 * id). Unary callers always receive typed errors immediately during
 * outages; the transport budget turns a silent Host into a typed
 * DEADLINE_EXCEEDED instead of a hang.
 */
export class ConnectionManager {
  private readonly deps: ConnectionDeps;
  private started = false;
  private stopRequested = false;
  private cancelTimer: (() => void) | null = null;

  constructor(deps: ConnectionDeps) {
    this.deps = deps;
  }

  /** Idempotent; safe to call from app-root effects and test setups. */
  async start(): Promise<void> {
    if (this.started) {
      return;
    }
    this.started = true;
    this.stopRequested = false;

    // Frame application and closure handling are global subscriptions; they
    // outlive individual streams by design.
    this.deps.onEventFrame((frame) => applyEventFrame(frame));
    this.deps.onEventsClosed((closed) => this.handleClosed(closed));

    const store = useConnectionStore.getState();
    store.patch({ phase: 'connecting', reconnectAttempt: 0 });
    await this.connectCycle();
  }

  /** Stops managing the connection and clears pending timers. */
  stop(): void {
    this.stopRequested = true;
    this.cancelTimer?.();
    this.cancelTimer = null;
    this.started = false;
    useConnectionStore.getState().patch({ phase: 'disconnected', reconnectAttempt: 0 });
  }

  /** Forces one immediate reconnect attempt (used by manual Retry UI). */
  async retryNow(): Promise<void> {
    this.cancelTimer?.();
    this.cancelTimer = null;
    if (!this.stopRequested) {
      await this.connectCycle();
    }
  }

  private async connectCycle(): Promise<void> {
    if (!this.started || this.stopRequested) {
      return;
    }
    // Open the stream first so the snapshot prefix repairs state as soon
    // as it arrives; the probe then fills version/health/methods.
    try {
      const lastOutage = useConnectionStore.getState().outageId;
      await this.deps.openEvents(lastOutage);
    } catch {
      // Pump already active means a previous stream still lives; its data
      // keeps flowing, so nothing else is needed here.
    }
    try {
      const probe = await this.deps.client.probeHost();
      this.applyProbe(probe);
    } catch (error) {
      this.recordFailure(error);
      this.scheduleReconnect(errorCodeOf(error));
    }
  }

  private applyProbe(probe: HostProbe): void {
    const previousPhase = useConnectionStore.getState().phase;
    const wasDown =
      previousPhase === 'reconnecting' ||
      previousPhase === 'auth-failed' ||
      previousPhase === 'connecting';
    const nextPhase: ConnectionPhase =
      probe.servingStatus === 'SERVING' ? 'authenticated' : 'degraded';
    useConnectionStore.getState().patch({
      phase: nextPhase,
      hostVersion: probe.hostVersion,
      capabilities: probe.capabilities,
      startedAtUnixMs: probe.startedAtUnixMs,
      servingStatus: probe.servingStatus,
      methods: probe.methods,
      lastErrorCode: null,
      lastErrorMessage: null,
      reconnectAttempt: 0,
    });
    if (wasDown && nextPhase === 'authenticated') {
      pushToast({
        kind: 'info',
        title: 'Connected to the Host',
        detail: `lazarus-hostd ${probe.hostVersion}`,
      });
    }
  }

  private recordFailure(error: unknown): void {
    useConnectionStore.getState().patch({
      lastErrorCode: errorCodeOf(error),
      lastErrorMessage: errorMessageOf(error),
    });
  }

  /**
   * A live stream ended. Only transitions away from a healthy phase act -
   * closures during recovery are echoes of attempts this loop already owns,
   * so ignoring them prevents duplicate timers and toast storms.
   */
  private handleClosed(closed: { reason: string; detail?: string | null }): void {
    if (!this.started || this.stopRequested) {
      useConnectionStore.getState().patch({ phase: 'disconnected', reconnectAttempt: 0 });
      return;
    }
    const phase = useConnectionStore.getState().phase;
    if (!HEALTHY_PHASES.has(phase)) {
      return;
    }
    pushToast({
      kind: 'error',
      title: 'Host connection lost',
      detail: closed.detail ?? closed.reason,
    });
    useConnectionStore.getState().patch({ phase: 'reconnecting', reconnectAttempt: 0 });
    this.scheduleReconnect(null);
  }

  private scheduleReconnect(errorCode: string | null): void {
    if (!this.started || this.stopRequested) {
      return;
    }
    const store = useConnectionStore.getState();
    const attempt = store.reconnectAttempt + 1;
    const authProblem = errorCode === 'UNAUTHENTICATED';
    store.patch({
      phase: authProblem ? 'auth-failed' : 'reconnecting',
      reconnectAttempt: attempt,
    });
    if (attempt === 1) {
      pushToast({
        kind: 'error',
        title: authProblem ? 'The Host rejected the local token' : 'Cannot reach the Host',
        detail: authProblem
          ? 'Start the Host with `lazarus host start` so the per-install token matches.'
          : 'Retrying automatically.',
      });
    }
    const delay = nextBackoffMs(attempt - 1);
    this.cancelTimer = this.deps.scheduleDelay(delay, () => {
      this.cancelTimer = null;
      void this.connectCycle();
    });
  }
}

function errorCodeOf(error: unknown): string | null {
  if (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    typeof (error as { code?: unknown }).code === 'string'
  ) {
    return (error as { code: string }).code;
  }
  return null;
}

function errorMessageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
