import {
  useEffect,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';

import { useEpicsStore } from '../../state/epics-store';
import type { TileBinding } from '../../lib/canvas/split-tree';

const KIND_TITLES: Record<TileBinding['kind'], string> = {
  chat: 'Chat agent',
  terminal: 'Terminal agent',
  artifact: 'Artifact',
};

/**
 * The Phase 3.4 tile surface. Real feature tiles replace this placeholder
 * in later phases; what matters now is that every tile bound to an entity
 * renders live shared state - committing an edit in one tile is visible in
 * every other tile bound to the same entity.
 */
export function TilePlaceholder({ binding }: { binding: TileBinding }): ReactNode {
  const entity = useEpicsStore((state) => state.epics[binding.entityId]);
  const renameEpic = useEpicsStore((state) => state.renameEpic);
  const [draft, setDraft] = useState(entity?.title ?? '');

  // Follow committed external renames while the field is not being edited.
  useEffect(() => {
    setDraft(entity?.title ?? '');
  }, [entity?.title]);

  const commit = (): void => {
    if (entity !== undefined && draft.trim() !== '' && draft !== entity.title) {
      renameEpic(entity.id, draft);
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>): void => {
    if (event.key === 'Enter') {
      event.currentTarget.blur();
    }
  };

  return (
    <div className="tile-placeholder" data-testid={`tile-content-${binding.id}`}>
      <div className="tile-placeholder-head">
        <span className="pill pill-starting">{KIND_TITLES[binding.kind]}</span>
        <span className="muted">{entity?.id ?? binding.entityId}</span>
      </div>
      <p className="tile-placeholder-note">
        {KIND_TITLES[binding.kind]} sessions arrive with provider adapters; this tile tracks its
        Epic.
      </p>
      <label className="tile-placeholder-field">
        Epic title
        <input
          value={draft}
          data-testid={`tile-title-input-${binding.id}`}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={commit}
          onKeyDown={onKeyDown}
        />
      </label>
      <p className="muted tile-placeholder-hint">
        Closing this tile never deletes the Epic - reopen it from Home or History.
      </p>
    </div>
  );
}
