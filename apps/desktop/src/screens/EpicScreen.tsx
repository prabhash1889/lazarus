import { useParams } from '@tanstack/react-router';
import { useCallback, useEffect, type ReactNode } from 'react';

import { TileCanvas } from '../components/canvas/TileCanvas';
import { TilePlaceholder } from '../components/canvas/TilePlaceholder';
import { freshId, type TileBinding, type TileKind } from '../lib/canvas/split-tree';
import { useTaskLayout } from '../lib/canvas/use-task-layout';
import { useEpicsStore } from '../state/epics-store';

/**
 * The Epic tab surface (Phase 3.4): the empty Epic canvas rendered inside
 * the tab strip as the first real canvas content. The layout document is
 * loaded from - and autosaved to - the Task's Host-persisted layout record,
 * so tabs and splits restore exactly across Desktop restarts.
 */
export default function EpicScreen(): ReactNode {
  const params = useParams({ strict: false });
  const taskId = params.taskId ?? '';
  const entity = useEpicsStore((state) => state.epics[taskId]);
  const { doc, change } = useTaskLayout(taskId);

  // Deep links can name an Epic this session has never seen; seed a stub
  // entity so tiles have something durable-looking to bind to until
  // Phase 8 makes Tasks real.
  useEffect(() => {
    if (taskId !== '' && useEpicsStore.getState().epics[taskId] === undefined) {
      useEpicsStore.getState().putEpic({
        id: taskId,
        title: `Epic ${taskId.slice(-6)}`,
        createdAt: Date.now(),
      });
    }
  }, [taskId]);

  const createTile = useCallback(
    (kind: TileKind): TileBinding => ({
      id: freshId('tile'),
      entityId: taskId,
      kind,
    }),
    [taskId],
  );

  if (doc === null) {
    return (
      <main className="epic-shell">
        <p className="muted" data-testid="epic-loading">
          Restoring canvas…
        </p>
      </main>
    );
  }

  return (
    <main className="epic-shell">
      <header className="epic-header">
        <h1>{entity?.title ?? 'Epic'}</h1>
        <span className="muted">{taskId}</span>
      </header>
      <TileCanvas
        doc={doc}
        onChange={change}
        renderTile={(binding) => <TilePlaceholder binding={binding} />}
        createTile={createTile}
      />
    </main>
  );
}
