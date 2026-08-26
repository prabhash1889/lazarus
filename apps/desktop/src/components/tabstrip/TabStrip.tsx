import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';

import { joinClassNames } from '../Button';
import { handleRovingKeys, rovingTabIndex } from '../../lib/a11y/roving-tabindex';
import { useEpicsStore } from '../../state/epics-store';
import { PINNED_TABS, useShellStore, type TabId } from '../../state/shell-store';
import { computeTargetIndex, planOverflow, type TabRect } from './tabstrip-model';

/**
 * The persistent header tab strip (Phase 3.4): pinned system tabs
 * (Draft | History | Settings) plus one tab per open Epic. Epics support
 * middle-click and affordance closing - which never deletes the backing
 * entity - pointer drag reordering, an overflow menu when the strip runs
 * out of room, and roving-tabindex keyboard navigation (Phase 3.5).
 */

export const DRAG_TILE_MIME = 'application/x-lazarus-tab-id';

const PINNED_LABELS: Record<(typeof PINNED_TABS)[number], string> = {
  draft: 'Draft',
  history: 'History',
  settings: 'Settings',
};

export function tabPath(id: TabId): string {
  if (id === 'draft') return '/draft';
  if (id === 'history') return '/history';
  if (id === 'settings') return '/settings';
  return `/epic/${id}`;
}

interface TabStripProps {
  activeTab: TabId;
  onSelect(id: TabId): void;
  onClose(epicId: string): void;
  onReorder(from: number, to: number): void;
  onNewEpic(): void;
}

export function TabStrip(props: TabStripProps): ReactNode {
  const { activeTab, onSelect, onClose, onReorder, onNewEpic } = props;
  const epicTabs = useShellStore((state) => state.epicTabs);
  const epics = useEpicsStore((state) => state.epics);

  const [menuOpen, setMenuOpen] = useState(false);
  const [inlineCount, setInlineCount] = useState(epicTabs.length);
  const [dragging, setDragging] = useState(false);
  // Roving tabindex state: which tab is the single tab stop of the list.
  const [focusIndex, setFocusIndex] = useState(0);
  const tablistRef = useRef<HTMLDivElement | null>(null);
  const tabRefs = useRef<Array<HTMLButtonElement | HTMLDivElement | null>>([]);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const measureRefs = useRef<Array<HTMLDivElement | null>>([]);
  const drag = useRef<{ index: number; startX: number; active: boolean } | null>(null);

  const visible = epicTabs.slice(0, Math.max(0, inlineCount));
  const hidden = epicTabs.slice(Math.max(0, inlineCount));
  const tabIds: TabId[] = [...PINNED_TABS, ...visible];

  // Keep the roving tab stop on the selected tab whenever selection moves.
  useEffect(() => {
    const ids: TabId[] = [...PINNED_TABS, ...epicTabs.slice(0, Math.max(0, inlineCount))];
    const index = ids.indexOf(activeTab);
    if (index >= 0) {
      setFocusIndex(index);
    }
  }, [activeTab, epicTabs, inlineCount]);

  // Measure overflow whenever the tab set changes or the window resizes.
  useLayoutEffect(() => {
    const measure = (): void => {
      const container = containerRef.current;
      if (container === null) {
        return;
      }
      const available = container.getBoundingClientRect().width;
      const widths = epicTabs.map((_, index) => {
        const el = measureRefs.current[index];
        return el?.getBoundingClientRect().width ?? 0;
      });
      setInlineCount(planOverflow(available, widths).inlineCount);
    };
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [epicTabs]);

  useEffect(() => {
    if (menuOpen) {
      const dismiss = (event: MouseEvent): void => {
        if (
          containerRef.current !== null &&
          event.target instanceof Node &&
          !containerRef.current.contains(event.target)
        ) {
          setMenuOpen(false);
        }
      };
      const escape = (event: KeyboardEvent): void => {
        if (event.key === 'Escape') {
          setMenuOpen(false);
        }
      };
      document.addEventListener('mousedown', dismiss);
      document.addEventListener('keydown', escape);
      return () => {
        document.removeEventListener('mousedown', dismiss);
        document.removeEventListener('keydown', escape);
      };
    }
    return undefined;
  }, [menuOpen]);

  const startDrag = (event: ReactPointerEvent<HTMLElement>, index: number): void => {
    if (event.button !== 0 || visible.length < 2) {
      return;
    }
    drag.current = { index, startX: event.clientX, active: false };
  };

  const moveDrag = (event: ReactPointerEvent<HTMLElement>): void => {
    const state = drag.current;
    if (state === null) {
      return;
    }
    if (!state.active && Math.abs(event.clientX - state.startX) > 4) {
      state.active = true;
      setDragging(true);
    }
    if (!state.active) {
      return;
    }
    const layout: TabRect[] = visible.map((id, index) => {
      const el = measureRefs.current[index];
      const rect = el?.getBoundingClientRect();
      return { id, left: rect?.left ?? 0, right: rect?.right ?? 0 };
    });
    const target = computeTargetIndex(layout, state.index, event.clientX);
    if (target !== state.index) {
      onReorder(state.index, target);
      drag.current = { ...state, index: target };
    }
  };

  const endDrag = (): void => {
    drag.current = null;
    setDragging(false);
  };

  // Manual activation: arrows and Home/End move the roving tab stop;
  // Enter/Space/click activate the focused tab (native button behavior).
  const onTablistKeyDown = (event: ReactKeyboardEvent<HTMLElement>): void => {
    handleRovingKeys(event, {
      count: tabIds.length,
      current: focusIndex,
      orientation: 'horizontal',
      onMove: (next) => {
        setFocusIndex(next);
        tabRefs.current[next]?.focus();
      },
    });
  };

  const renderPinnedTab = (id: (typeof PINNED_TABS)[number], index: number): ReactNode => (
    <button
      key={id}
      ref={(el) => {
        tabRefs.current[index] = el;
      }}
      type="button"
      role="tab"
      aria-selected={activeTab === id}
      tabIndex={rovingTabIndex(index, focusIndex)}
      className={joinClassNames('shell-tab', activeTab === id && 'shell-tab-active')}
      data-testid={`tab-${id}`}
      onFocus={() => setFocusIndex(index)}
      onClick={() => onSelect(id)}
    >
      {PINNED_LABELS[id]}
    </button>
  );

  const renderEpicTab = (epicId: string, index: number): ReactNode => {
    const entity = epics[epicId];
    const isActive = activeTab === epicId;
    const tabIndex = rovingTabIndex(PINNED_TABS.length + index, focusIndex);
    return (
      <div
        key={epicId}
        ref={(el) => {
          measureRefs.current[index] = el;
          tabRefs.current[PINNED_TABS.length + index] = el;
        }}
        className={joinClassNames(
          'shell-tab',
          'shell-tab-epic',
          isActive && 'shell-tab-active',
          dragging && drag.current?.index === index && 'shell-tab-dragging',
        )}
        data-testid={`tab-${epicId}`}
      >
        <button
          type="button"
          role="tab"
          aria-selected={isActive}
          tabIndex={tabIndex}
          title={entity?.title ?? epicId}
          className="shell-tab-label"
          onFocus={() => setFocusIndex(PINNED_TABS.length + index)}
          onAuxClick={(event) => {
            // Middle-click closes without ever touching the entity.
            if (event.button === 1) {
              event.preventDefault();
              onClose(epicId);
            }
          }}
          onMouseDown={(event) => {
            // Browsers fire autoscroll on middle mousedown; closing here
            // both feels instant and suppresses that default.
            if (event.button === 1) {
              event.preventDefault();
              onClose(epicId);
            }
          }}
          onClick={() => onSelect(epicId)}
          onPointerDown={(event) => startDrag(event, index)}
          onPointerMove={moveDrag}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onKeyDown={(event) => {
            if (event.key === 'Delete') {
              onClose(epicId);
            }
          }}
        >
          <span className="shell-tab-title">{entity?.title ?? epicId}</span>
        </button>
        <button
          type="button"
          aria-label={`Close ${entity?.title ?? 'Epic'} tab`}
          className="shell-tab-close"
          data-testid={`close-tab-${epicId}`}
          onClick={(event) => {
            event.stopPropagation();
            onClose(epicId);
          }}
        >
          ×
        </button>
      </div>
    );
  };

  return (
    <div className="tabstrip" ref={containerRef} data-testid="tab-strip">
      <div
        ref={tablistRef}
        role="tablist"
        aria-label="Open tabs"
        className="tabstrip-tabs"
        data-testid="tab-list"
        onKeyDown={onTablistKeyDown}
      >
        {PINNED_TABS.map((id, index) => renderPinnedTab(id, index))}
        {visible.map((epicId, index) => renderEpicTab(epicId, index))}
      </div>
      {hidden.length > 0 ? (
        <div className="tabstrip-overflow">
          <button
            type="button"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            className="shell-tab shell-tab-overflow"
            data-testid="tab-overflow"
            onClick={() => setMenuOpen((open) => !open)}
          >
            +{hidden.length}
          </button>
          {menuOpen ? (
            <div className="tabstrip-menu" role="menu" aria-label="Hidden Epic tabs">
              {hidden.map((epicId) => (
                <button
                  key={epicId}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  className="tabstrip-menu-item"
                  data-testid={`overflow-item-${epicId}`}
                  onClick={() => {
                    setMenuOpen(false);
                    onSelect(epicId);
                  }}
                >
                  {epics[epicId]?.title ?? epicId}
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
      <button
        type="button"
        className="shell-tab shell-tab-new"
        aria-label="New Epic"
        title="New Epic"
        data-testid="new-epic"
        onClick={onNewEpic}
      >
        +
      </button>
    </div>
  );
}
