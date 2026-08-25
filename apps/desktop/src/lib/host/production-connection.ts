import { invokeCommand, listenToEvent } from '../tauri';
import { LazarusProtocolClient } from '../protocol/client';
import { ConnectionManager, type ConnectionDeps } from './connection-manager';

/**
 * The production wiring of the connection manager: the protocol client
 * singleton speaks through the Rust IPC bridge commands, and event frames
 * arrive as Tauri events from the Rust-side subscription pump.
 */

export const EVENT_FRAME_EVENT = 'lazarus://event-frame';
export const EVENTS_CLOSED_EVENT = 'lazarus://events-closed';

interface IpcResponseWire {
  ok: boolean;
  status: number | null;
  manifest: string | null;
  body: string | null;
  error: { code: string; message: string; retryable: boolean } | null;
}

function tauriDeps(): ConnectionDeps {
  const transport = {
    unary(args: { requestId: number; path: string; httpMethod?: string; payload?: string | null }) {
      return invokeCommand<IpcResponseWire>('host_ipc_request', {
        args: {
          requestId: args.requestId,
          path: args.path,
          httpMethod: args.httpMethod ?? null,
          payload: args.payload ?? null,
        },
      });
    },
    cancel(requestId: number) {
      return invokeCommand<boolean>('host_ipc_cancel', { requestId });
    },
  };
  const client = new LazarusProtocolClient(transport);
  return {
    client: {
      probeHost: (options) => client.probeHost(options),
    },
    openEvents: (lastOutageId) =>
      invokeCommand<void>('host_ipc_open_events', {
        args: { lastOutageId: lastOutageId ?? null },
      }),
    onEventFrame: (handler) => {
      let dispose = (): void => {};
      void listenToEvent<unknown>(EVENT_FRAME_EVENT, handler).then((off) => {
        dispose = off;
      });
      return () => dispose();
    },
    onEventsClosed: (handler) => {
      let dispose = (): void => {};
      void listenToEvent<{ reason: string; detail?: string | null }>(
        EVENTS_CLOSED_EVENT,
        handler,
      ).then((off) => {
        dispose = off;
      });
      return () => dispose();
    },
    scheduleDelay(ms, fn) {
      const handle = window.setTimeout(fn, ms);
      return () => window.clearTimeout(handle);
    },
  };
}

/** The app-wide connection manager singleton. */
export const connectionManager = new ConnectionManager(tauriDeps());

/** The app-wide protocol client for feature surfaces. */
export function protocolClient(): LazarusProtocolClient {
  return new LazarusProtocolClient({
    unary(args) {
      return invokeCommand<IpcResponseWire>('host_ipc_request', {
        args: {
          requestId: args.requestId,
          path: args.path,
          httpMethod: args.httpMethod ?? null,
          payload: args.payload ?? null,
        },
      });
    },
    cancel(requestId: number) {
      return invokeCommand<boolean>('host_ipc_cancel', { requestId });
    },
  });
}
