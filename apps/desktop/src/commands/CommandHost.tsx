import { useEffect, type ReactNode } from 'react';

import { CommandPalette } from '../components/CommandPalette';
import { findCommandBySequence, hasLongerSequencePrefix, runCommand } from './command-registry';
import { useRegisterAppCommands } from './default-commands';
import { ShortcutEngine, defaultShouldIgnore } from './shortcut-engine';

const engine = new ShortcutEngine({
  lookup: (sequence) => findCommandBySequence(sequence),
  hasExtension: (prefix) => hasLongerSequencePrefix(prefix),
  execute: (id) => runCommand(id),
  shouldIgnore: defaultShouldIgnore,
});

/**
 * Mounts the global command layer inside the routed shell: registers the
 * baseline commands, dispatches keyboard shortcuts, and hosts the palette.
 */
export function CommandHost(): ReactNode {
  useRegisterAppCommands();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) {
        return;
      }
      if (engine.handleKeyDown(event)) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      engine.reset();
    };
  }, []);

  return <CommandPalette />;
}

/** Clears buffered chord state between tests. */
export function engineResetForTests(): void {
  engine.reset();
}
