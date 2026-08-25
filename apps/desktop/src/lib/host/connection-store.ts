import { EventFrameSchema, type MethodSupport } from '@lazarus/protocol-ts';
import { create } from 'zustand';

import { pushToast } from '../../state/toast-store';
import type { NegotiatedMethodSummary } from '../protocol/client';

/**
 * The visible states of the Desktop's Host connection machine:
 *
 * ```
 * DISCONNECTED -> CONNECTING -> AUTHENTICATED
 *                  |  ^            |
 *                  v  |            v
 *             RECONNECTING <--- (stream lost)
 * AUTHENTICATED -> DEGRADED            (health NOT_SERVING)
 * any retry     -> AUTH-FAILED         (UNAUTHENTICATED)
 * ```
 */
export type ConnectionPhase =
  'disconnected' | 'connecting' | 'authenticated' | 'reconnecting' | 'auth-failed' | 'degraded';

export interface CapabilityEntry {
  name: string;
  enabled: boolean;
}

export interface WorkspaceSnapshot {
  id: string;
  name: string;
}

export interface TaskSnapshot {
  id: string;
  workspaceId: string;
  title: string;
}

export interface ConnectionState {
  phase: ConnectionPhase;
  /** Canonical code of the most recent failure, when one exists. */
  lastErrorCode: string | null;
  lastErrorMessage: string | null;
  hostVersion: string | null;
  startedAtUnixMs: number | null;
  servingStatus: 'SERVING' | 'NOT_SERVING' | null;
  capabilities: CapabilityEntry[];
  methods: NegotiatedMethodSummary[];
  /** Authoritative snapshot applied from the event stream. */
  workspaces: WorkspaceSnapshot[];
  tasks: TaskSnapshot[];
  liveSequence: number | null;
  /** Outage id of the current known Host incarnation. */
  outageId: string | null;
  reconnectAttempt: number;
}

export interface ConnectionStore extends ConnectionState {
  patch: (patch: Partial<ConnectionState>) => void;
  reset: () => void;
}

const initialState: ConnectionState = {
  phase: 'disconnected',
  lastErrorCode: null,
  lastErrorMessage: null,
  hostVersion: null,
  startedAtUnixMs: null,
  servingStatus: null,
  capabilities: [],
  methods: [],
  workspaces: [],
  tasks: [],
  liveSequence: null,
  outageId: null,
  reconnectAttempt: 0,
};

export const useConnectionStore = create<ConnectionStore>((set) => ({
  ...initialState,
  patch: (patch) => set(patch),
  reset: () => set({ ...initialState }),
}));

/**
 * Applies one raw event-frame payload to the store. Exported for direct
 * testing; the manager wires it to the transport's frame feed.
 */
export function applyEventFrame(raw: unknown): void {
  const frame = EventFrameSchema.safeParse(raw);
  if (!frame.success) {
    return; // Off-contract frames never touch state.
  }
  const store = useConnectionStore.getState();
  switch (frame.data.type) {
    case 'outage': {
      const previous = store.outageId;
      if (previous === frame.data.outageId) {
        // Restart tombstone deduplication: this incarnation is already
        // applied, so the redundant announcement changes nothing.
        return;
      }
      store.patch({
        outageId: frame.data.outageId,
        // A fresh incarnation invalidates any stale live sequence.
        liveSequence: null,
      });
      if (previous !== null) {
        pushToast({
          kind: 'info',
          title: 'The Host restarted',
          detail: 'Connection restored and state resynchronized.',
        });
      }
      return;
    }
    case 'snapshot':
      store.patch({
        workspaces: frame.data.workspaces.map((workspace) => ({ ...workspace })),
        tasks: frame.data.tasks.map((task) => ({ ...task })),
      });
      return;
    case 'live':
      store.patch({ liveSequence: frame.data.sequence });
      return;
  }
}

/** Convenience selectors used by surfaces. */
export function selectPhase(state: ConnectionState): ConnectionPhase {
  return state.phase;
}

export function selectMethodLabel(method: NegotiatedMethodSummary): string {
  if (method.support === 'supported' && method.version !== null) {
    return `${method.name}=${method.version}`;
  }
  if (method.support === 'fallback' && method.fallback !== null) {
    return `${method.name}=>${method.fallback} (fallback)`;
  }
  return `${method.name}=unavailable`;
}

export function supportLabel(support: MethodSupport): string {
  switch (support) {
    case 'supported':
      return 'supported';
    case 'fallback':
      return 'fallback';
    case 'unsupported':
      return 'unavailable';
  }
}
