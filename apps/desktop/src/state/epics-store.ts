import { create } from 'zustand';

import { freshId } from '../lib/canvas/split-tree';

/**
 * The stub entity registry (Phase 3.4). Durable Tasks arrive in Phase 8;
 * until then Epic entities live here so the shell can prove its
 * non-destructive contract: closing a tab or tile never removes the
 * backing entity, and every tile bound to one sees the same state.
 */

export interface EpicEntity {
  id: string;
  title: string;
  createdAt: number;
}

export interface EpicsState {
  epics: Record<string, EpicEntity>;
  createEpic(title?: string): EpicEntity;
  /** Seeds an entity that already has a durable-looking id (restores). */
  putEpic(entity: EpicEntity): void;
  renameEpic(id: string, title: string): void;
}

export const useEpicsStore = create<EpicsState>((set, get) => ({
  epics: {},
  createEpic(title?: string) {
    const entity: EpicEntity = {
      id: freshId('epic'),
      title: title?.trim() || 'Untitled Epic',
      createdAt: Date.now(),
    };
    set((state) => ({ epics: { ...state.epics, [entity.id]: entity } }));
    return entity;
  },
  putEpic(entity) {
    set((state) => ({ epics: { ...state.epics, [entity.id]: entity } }));
  },
  renameEpic(id, title) {
    const existing = get().epics[id];
    if (existing === undefined) {
      return;
    }
    set((state) => ({
      epics: {
        ...state.epics,
        [id]: { ...existing, title: title.trim() || existing.title },
      },
    }));
  },
}));

/** Test seam: reset the registry between tests. */
export function resetEpicsForTests(): void {
  useEpicsStore.setState({ epics: {} });
}
