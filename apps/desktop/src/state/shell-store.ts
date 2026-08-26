import { z } from 'zod';
import { create } from 'zustand';

import type { CanvasDoc } from '../lib/canvas/split-tree';
import { emptyCanvasDoc } from '../lib/canvas/split-tree';
import { useEpicsStore, type EpicEntity } from './epics-store';

/**
 * The shell-level UI state (Phase 3.4): which tabs are open, their order,
 * the active tab, and each Epic's canvas document.
 *
 * Pinned system tabs (Draft | History | Settings) always exist and cannot
 * close; Epic tabs are dynamic. Closing an Epic tab discards only its tab
 * and canvas view state - the entity itself stays in the epics registry and
 * the Host-persisted layout record keeps the arrangement for reopening.
 */

export type PinnedTabId = 'draft' | 'history' | 'settings';

export type TabId = PinnedTabId | string;

export const PINNED_TABS: readonly PinnedTabId[] = ['draft', 'history', 'settings'];

export function isPinnedTab(id: TabId): id is PinnedTabId {
  return (PINNED_TABS as readonly string[]).includes(id);
}

/** The persisted shell record: open Epics plus their order and activation. */
export interface ShellRecord {
  version: 1;
  epics: EpicEntity[];
  epicTabs: string[];
  activeTab: TabId | null;
  canvases: Record<string, CanvasDoc>;
}

const ShellRecordSchema = z.object({
  version: z.literal(1),
  epics: z.array(
    z.object({
      id: z.string().min(1),
      title: z.string().min(1),
      createdAt: z.number(),
    }),
  ),
  epicTabs: z.array(z.string().min(1)),
  activeTab: z.string().nullable(),
  canvases: z.record(z.string(), z.unknown()),
});

export interface ShellState {
  epicTabs: string[];
  activeTab: TabId;
  canvases: Record<string, CanvasDoc>;
  /** Revisions seen from the Host per task; drives optimistic writes. */
  revisions: Record<string, number>;
  /** Marks canvases whose local edits have not been acknowledged yet. */
  dirtyCanvases: Record<string, true>;

  openEpic(epicId: string): void;
  closeEpicTab(epicId: string): void;
  setActiveTab(tab: TabId): void;
  moveTab(from: number, to: number): void;
  setCanvas(taskId: string, doc: CanvasDoc): void;
  markCanvasClean(taskId: string, revision: number): void;
  markCanvasDirty(taskId: string): void;
}

export const initialShellState = {
  epicTabs: [],
  activeTab: 'draft' as TabId,
  canvases: {},
  revisions: {},
  dirtyCanvases: {},
};

export const useShellStore = create<ShellState>((set, get) => ({
  ...initialShellState,

  openEpic(epicId) {
    const { epicTabs } = get();
    set({
      epicTabs: epicTabs.includes(epicId) ? epicTabs : [...epicTabs, epicId],
      activeTab: epicId,
    });
  },

  closeEpicTab(epicId) {
    const { epicTabs, activeTab, canvases } = get();
    const nextTabs = epicTabs.filter((id) => id !== epicId);
    const nextCanvases = { ...canvases };
    delete nextCanvases[epicId];
    // Closing a tab is non-destructive to the entity; only the view goes.
    // The persisted layout record is left in place so reopening restores it.
    set({
      epicTabs: nextTabs,
      canvases: nextCanvases,
      activeTab: activeTab === epicId ? (nextTabs.at(-1) ?? 'draft') : activeTab,
    });
  },

  setActiveTab(tab) {
    if (!isPinnedTab(tab) && !get().epicTabs.includes(tab)) {
      return;
    }
    set({ activeTab: tab });
  },

  moveTab(from, to) {
    const { epicTabs } = get();
    const count = epicTabs.length;
    if (from < 0 || from >= count || to < 0 || to >= count || from === to) {
      return;
    }
    const next = [...epicTabs];
    const [moved] = next.splice(from, 1);
    if (moved === undefined) {
      return;
    }
    next.splice(to, 0, moved);
    set({ epicTabs: next });
  },

  setCanvas(taskId, doc) {
    set((state) => ({
      canvases: { ...state.canvases, [taskId]: doc },
      dirtyCanvases: { ...state.dirtyCanvases, [taskId]: true },
    }));
  },

  markCanvasClean(taskId, revision) {
    set((state) => {
      const dirtyCanvases = { ...state.dirtyCanvases };
      delete dirtyCanvases[taskId];
      return {
        revisions: { ...state.revisions, [taskId]: revision },
        dirtyCanvases,
      };
    });
  },

  markCanvasDirty(taskId) {
    set((state) => ({
      dirtyCanvases: { ...state.dirtyCanvases, [taskId]: true },
    }));
  },
}));

/**
 * Restores shell state from a persisted record. Entities re-enter the
 * epics registry; unknown canvases are dropped; the active tab falls back
 * sensibly when it names nothing open. Anything malformed restores nothing.
 */
export function hydrateShellFromRecord(record: ShellRecord): boolean {
  const decoded = ShellRecordSchema.safeParse(record);
  if (!decoded.success) {
    return false;
  }
  const data = decoded.data;
  const epics = useEpicsStore.getState();
  for (const entity of data.epics) {
    epics.putEpic(entity);
  }
  const knownIds = new Set(data.epics.map((entity) => entity.id));
  const epicTabs = [...new Set(data.epicTabs)].filter((id) => knownIds.has(id));
  const canvases: Record<string, CanvasDoc> = {};
  for (const [taskId, doc] of Object.entries(data.canvases)) {
    if (knownIds.has(taskId)) {
      canvases[taskId] = doc as CanvasDoc;
    }
  }
  const activeTab: TabId =
    data.activeTab !== null && (isPinnedTab(data.activeTab) || epicTabs.includes(data.activeTab))
      ? data.activeTab
      : (epicTabs.at(-1) ?? 'draft');
  useShellStore.setState({
    epicTabs,
    canvases,
    activeTab,
    revisions: {},
    dirtyCanvases: {},
  });
  return true;
}

/** Snapshots current shell state into its persistable form. */
export function shellToRecord(): ShellRecord {
  const state = useShellStore.getState();
  const epics = Object.values(useEpicsStore.getState().epics).filter((entity) =>
    state.epicTabs.includes(entity.id),
  );
  return {
    version: 1,
    epics,
    epicTabs: state.epicTabs,
    activeTab: state.activeTab,
    canvases: state.canvases,
  };
}

/** Ensures a canvas document exists for a task, seeding an empty one. */
export function ensureCanvas(taskId: string): CanvasDoc {
  const existing = useShellStore.getState().canvases[taskId];
  if (existing !== undefined) {
    return existing;
  }
  const doc = emptyCanvasDoc();
  useShellStore.setState((state) => ({
    canvases: { ...state.canvases, [taskId]: doc },
  }));
  return doc;
}

/**
 * Creates a stub Epic entity and opens its tab. Durable Task creation
 * replaces this in Phase 8; until then this is how new Epic tabs appear.
 */
export function createStubEpic(title?: string): EpicEntity {
  const entity = useEpicsStore.getState().createEpic(title);
  useShellStore.getState().openEpic(entity.id);
  return entity;
}

/** Test seam: reset both stores between tests. */
export function resetShellForTests(): void {
  useShellStore.setState({ ...initialShellState });
}
