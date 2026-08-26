import * as DialogPrimitive from '@radix-ui/react-dialog';
import { useMemo, useRef, useState, useEffect, type ReactNode } from 'react';

import { getAvailableCommands, runCommand, useCommandRegistry } from '../commands/command-registry';
import { searchCommands } from '../commands/search';
import { formatShortcut, parseShortcut } from '../commands/shortcut-keys';
import type { RegisteredCommand } from '../commands/types';
import { usePaletteStore } from '../state/palette-store';
import { VirtualList, buildGroupedRows, type VirtualListHandle } from './VirtualList';

interface ListRow {
  command: RegisteredCommand;
}

function groupLabel(command: RegisteredCommand): string | null {
  return command.section ?? null;
}

export function CommandPalette(): ReactNode {
  const open = usePaletteStore((state) => state.open);
  const closePalette = usePaletteStore((state) => state.closePalette);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const listRef = useRef<VirtualListHandle | null>(null);

  // Snapshot commands per open/query so availability predicates re-evaluate.
  const usage = useCommandRegistry((state) => state.usage);
  const rows = useMemo(() => {
    if (!open) {
      return [];
    }
    const ranked = searchCommands(query, getAvailableCommands(), usage, Date.now());
    const items = ranked.map((entry) => ({ command: entry.command }));
    return buildGroupedRows<ListRow>(
      items,
      (row) => groupLabel(row.command),
      (row) => row.command.id,
    );
  }, [open, query, usage]);

  useEffect(() => {
    if (!open) {
      setQuery('');
      setActiveIndex(null);
    }
  }, [open]);

  useEffect(() => {
    setActiveIndex(rows.length > 0 ? firstItemIndex(rows) : null);
  }, [rows]);

  useEffect(() => {
    if (open && activeIndex !== null) {
      listRef.current?.scrollToIndex(activeIndex, 'auto');
    }
  }, [activeIndex, open]);

  function firstItemIndex(list: ReturnType<typeof buildGroupedRows<ListRow>>): number | null {
    for (let index = 0; index < list.length; index += 1) {
      if (list[index]?.kind === 'item') {
        return index;
      }
    }
    return null;
  }

  const itemRowIndexes = useMemo(
    () =>
      rows.reduce<number[]>((acc, row, index) => {
        if (row.kind === 'item') {
          acc.push(index);
        }
        return acc;
      }, []),
    [rows],
  );

  function moveActive(step: number): void {
    if (itemRowIndexes.length === 0) {
      return;
    }
    const position = activeIndex === null ? -1 : itemRowIndexes.indexOf(activeIndex);
    const nextPosition =
      position < 0
        ? step === 1
          ? 0
          : itemRowIndexes.length - 1
        : Math.min(itemRowIndexes.length - 1, Math.max(0, position + step));
    const nextIndex = itemRowIndexes[nextPosition];
    if (nextIndex !== undefined) {
      setActiveIndex(nextIndex);
    }
  }

  function handleInputKeyDown(event: React.KeyboardEvent<HTMLInputElement>): void {
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        moveActive(1);
        break;
      case 'ArrowUp':
        event.preventDefault();
        moveActive(-1);
        break;
      case 'Home':
        event.preventDefault();
        moveActive(-itemRowIndexes.length);
        break;
      case 'End':
        event.preventDefault();
        moveActive(itemRowIndexes.length);
        break;
      case 'Enter': {
        event.preventDefault();
        if (activeIndex !== null) {
          execute(activeIndex);
        }
        break;
      }
      default:
        break;
    }
  }

  function execute(index: number): void {
    const row = rows[index];
    if (!row || row.kind !== 'item') {
      return;
    }
    const commandId = row.item.command.id;
    closePalette();
    runCommand(commandId);
  }

  function shortcutLabel(command: RegisteredCommand): string | null {
    if (!command.shortcut || command.shortcutRejectedBy) {
      return null;
    }
    try {
      return formatShortcut(parseShortcut(command.shortcut));
    } catch {
      return null;
    }
  }

  return (
    <DialogPrimitive.Root open={open} onOpenChange={(next) => (next ? undefined : closePalette())}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="dialog-overlay palette-overlay" />
        <DialogPrimitive.Content className="palette-content" aria-describedby={undefined}>
          <DialogPrimitive.Title className="palette-title">Command palette</DialogPrimitive.Title>
          <input
            className="palette-input"
            data-testid="palette-input"
            placeholder="Type a command..."
            aria-label="Search commands"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={handleInputKeyDown}
            autoComplete="off"
            spellCheck={false}
          />
          {rows.length === 0 ? (
            <p className="muted palette-empty" role="status">
              No matching commands.
            </p>
          ) : (
            <VirtualList<ListRow>
              ref={listRef}
              className="palette-list"
              viewportHeight={320}
              rows={rows}
              rowHeight={(row) => (row.kind === 'header' ? 28 : 40)}
              ariaLabel="Commands"
              activeIndex={activeIndex}
              onActiveIndexChange={setActiveIndex}
              onActivateItem={(_row, index) => execute(index)}
              renderItem={(row, _index, { active }) => {
                const binding = shortcutLabel(row.command);
                return (
                  <div
                    className={active ? 'palette-item palette-item-active' : 'palette-item'}
                    data-testid="palette-command"
                    data-command-id={row.command.id}
                  >
                    <span className="palette-item-title">{row.command.title}</span>
                    {binding !== null ? <kbd className="palette-kbd">{binding}</kbd> : null}
                  </div>
                );
              }}
            />
          )}
          <div className="palette-footer">
            <span>
              <kbd>Up</kbd> <kbd>Down</kbd> navigate
            </span>
            <span>
              <kbd>Enter</kbd> run
            </span>
            <span>
              <kbd>Esc</kbd> close
            </span>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
