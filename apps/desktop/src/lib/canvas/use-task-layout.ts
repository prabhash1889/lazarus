import { useCallback, useEffect, useRef } from 'react';

import { pushToast } from '../../state/toast-store';
import { ensureCanvas, useShellStore } from '../../state/shell-store';
import type { CanvasDoc } from './split-tree';
import { loadTaskLayout, saveTaskLayout, taskLayoutGateway } from './task-layout-persistence';

/**
 * Wires one Epic's canvas document to its Host-persisted layout record:
 * loads once on mount, autosaves debounced edits with optimistic revision
 * guards, and degrades to session-only state when the Host cannot serve
 * layouts. The Host record is authoritative for restoration; the active
 * window owns concurrent conflict resolution (single-user local product).
 */

const AUTOSAVE_DELAY_MS = 500;
const CONFLICT_RETRY_MS = 250;

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** Toast-once bookkeeping so a degraded Host does not spam the viewport. */
const unavailableWarned = new Set<string>();

function clearTimer(taskId: string): void {
  const existing = timers.get(taskId);
  if (existing !== undefined) {
    clearTimeout(existing);
    timers.delete(taskId);
  }
}

export function scheduleCanvasSave(
  gateway: Parameters<typeof saveTaskLayout>[0],
  taskId: string,
): void {
  clearTimer(taskId);
  timers.set(
    taskId,
    setTimeout(() => {
      timers.delete(taskId);
      void flushCanvasSave(gateway, taskId);
    }, AUTOSAVE_DELAY_MS),
  );
}

async function flushCanvasSave(
  gateway: Parameters<typeof saveTaskLayout>[0],
  taskId: string,
): Promise<void> {
  const state = useShellStore.getState();
  const doc = state.canvases[taskId];
  if (doc === undefined || state.dirtyCanvases[taskId] !== true) {
    return;
  }
  const outcome = await saveTaskLayout(gateway, taskId, doc, state.revisions[taskId]);
  if (outcome.status === 'saved') {
    useShellStore.getState().markCanvasClean(taskId, outcome.revision);
    return;
  }
  if (outcome.status === 'conflict') {
    // Another writer won the race; retry unguarded after a beat so the
    // active window's arrangement converges without user intervention.
    timers.set(
      taskId,
      setTimeout(() => {
        timers.delete(taskId);
        const current = useShellStore.getState().canvases[taskId];
        if (current === undefined) {
          return;
        }
        void saveTaskLayout(gateway, taskId, current, undefined).then((retry) => {
          if (retry.status === 'saved') {
            useShellStore.getState().markCanvasClean(taskId, retry.revision);
          }
        });
      }, CONFLICT_RETRY_MS),
    );
    return;
  }
  // Unavailable: stay dirty; the next edit or mount retries.
}

export interface TaskLayoutBinding {
  doc: CanvasDoc | null;
  change(next: CanvasDoc): void;
}

export function useTaskLayout(
  taskId: string,
  gateway: Parameters<typeof loadTaskLayout>[0] = taskLayoutGateway,
): TaskLayoutBinding {
  const doc = useShellStore((state) => state.canvases[taskId] ?? null);
  const loadedOnce = useRef(false);

  // Initial load: seed the canvas from the Host record exactly as it was
  // left, or start empty when nothing durable exists.
  useEffect(() => {
    let cancelled = false;
    loadedOnce.current = false;
    void loadTaskLayout(gateway, taskId).then((result) => {
      if (cancelled) {
        return;
      }
      loadedOnce.current = true;
      if (result.status === 'loaded' && result.doc !== null) {
        useShellStore.getState().setCanvas(taskId, result.doc);
        useShellStore.getState().markCanvasClean(taskId, result.revision);
        return;
      }
      ensureCanvas(taskId);
      if (result.status === 'unavailable' && !unavailableWarned.has(taskId)) {
        unavailableWarned.add(taskId);
        pushToast({
          kind: 'info',
          title: 'Layout persistence unavailable',
          detail:
            'The Host cannot store this Epic\u2019s layout right now; tabs and splits last for this session.',
        });
      }
    });
    return () => {
      cancelled = true;
    };
  }, [gateway, taskId]);

  // Final flush on teardown so a quick edit never dies with the timer.
  useEffect(() => {
    return () => {
      const pending = timers.get(taskId);
      if (pending !== undefined) {
        clearTimeout(pending);
        timers.delete(taskId);
        void flushCanvasSave(gateway, taskId);
      }
    };
  }, [gateway, taskId]);

  const change = useCallback(
    (next: CanvasDoc) => {
      useShellStore.getState().setCanvas(taskId, next);
      scheduleCanvasSave(gateway, taskId);
    },
    [gateway, taskId],
  );

  return { doc, change };
}
