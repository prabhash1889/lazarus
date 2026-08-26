import { RequestCancelledError, isProtocolCallError } from '../protocol/errors';
import { protocolClient } from '../host/production-connection';
import { parseCanvasDoc, serializeCanvasDoc, type CanvasDoc } from './split-tree';

/**
 * Per-Task layout persistence (Phase 3.4): the Desktop stores each Task's
 * shell document through the Host's `task.layout.*` records so tabs and
 * splits restore exactly across restarts.
 *
 * The methods are optional in the contract, so every failure degrades to
 * session-only state instead of breaking the canvas: an older Host or a
 * dropped connection yields `unavailable`, never a crash.
 */

/** Transport seam over the layout records; production speaks the protocol. */
export interface TaskLayoutGateway {
  load(taskId: string): Promise<GatewayRecord>;
  save(
    taskId: string,
    layoutJson: string,
    expectedRevision: number | undefined,
  ): Promise<GatewayRevision>;
}

export type GatewayRecord =
  | { ok: true; layoutJson: string | null; revision: number }
  | { ok: false; reason: 'unavailable'; message: string };

export type GatewayRevision =
  | { ok: true; revision: number }
  | { ok: false; reason: 'conflict' | 'unavailable'; message: string };

export interface LoadedTaskLayout {
  status: 'loaded' | 'missing' | 'unavailable';
  doc: CanvasDoc | null;
  revision: number;
}

export type SavedTaskLayout =
  | { status: 'saved'; revision: number }
  | { status: 'conflict' }
  | { status: 'unavailable'; message: string };

export function loadTaskLayout(
  gateway: TaskLayoutGateway,
  taskId: string,
): Promise<LoadedTaskLayout> {
  return gateway.load(taskId).then((record) => {
    if (!record.ok) {
      return { status: 'unavailable', doc: null, revision: 0 } as const;
    }
    if (record.layoutJson === null || record.revision === 0) {
      return { status: 'missing', doc: null, revision: record.revision } as const;
    }
    // A corrupt or off-model document degrades to "no layout" instead of
    // poisoning the canvas; the Host keeps the raw bytes until the next
    // successful save replaces them.
    return {
      status: 'loaded',
      doc: parseCanvasDoc(record.layoutJson),
      revision: record.revision,
    } as const;
  });
}

export function saveTaskLayout(
  gateway: TaskLayoutGateway,
  taskId: string,
  doc: CanvasDoc,
  expectedRevision: number | undefined,
): Promise<SavedTaskLayout> {
  return gateway.save(taskId, serializeCanvasDoc(doc), expectedRevision).then((outcome) => {
    if (outcome.ok) {
      return { status: 'saved', revision: outcome.revision } as const;
    }
    if (outcome.reason === 'conflict') {
      return { status: 'conflict' } as const;
    }
    return { status: 'unavailable', message: outcome.message } as const;
  });
}

function unavailable(error: unknown): { ok: false; reason: 'unavailable'; message: string } {
  const detail = isProtocolCallError(error)
    ? `${error.code}: ${error.message}`
    : error instanceof Error
      ? error.message
      : 'unknown transport failure';
  return { ok: false, reason: 'unavailable', message: detail };
}

/** Minimal client surface the production gateway needs. */
export interface LayoutProtocolClient {
  call(methodName: string, input: unknown): Promise<unknown>;
}

/**
 * Production gateway speaking the Lazarus Protocol through the IPC bridge.
 * The client factory is injectable so tests can drive a fake transport.
 */
export function hostTaskLayoutGateway(
  clientFactory: () => LayoutProtocolClient = protocolClient,
): TaskLayoutGateway {
  return {
    async load(taskId) {
      try {
        const response = (await clientFactory().call('task.layout.get', { taskId })) as {
          layoutJson?: string;
          revision: number;
        };
        return {
          ok: true,
          layoutJson: response.layoutJson ?? null,
          revision: response.revision,
        };
      } catch (error) {
        if (error instanceof RequestCancelledError) {
          throw error;
        }
        return unavailable(error);
      }
    },
    async save(taskId, layoutJson, expectedRevision) {
      try {
        const response = (await clientFactory().call(
          'task.layout.put',
          expectedRevision === undefined
            ? { taskId, layoutJson }
            : { taskId, layoutJson, expectedRevision },
        )) as { revision: number };
        return { ok: true, revision: response.revision };
      } catch (error) {
        if (error instanceof RequestCancelledError) {
          throw error;
        }
        if (isProtocolCallError(error) && error.code === 'FAILED_PRECONDITION') {
          return {
            ok: false,
            reason: 'conflict',
            message: error.message,
          };
        }
        return unavailable(error);
      }
    },
  };
}

/** The app-wide production gateway. */
export const taskLayoutGateway = hostTaskLayoutGateway();
