import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  findCommandBySequence,
  getAvailableCommands,
  hasLongerSequencePrefix,
  runCommand,
  useCommandRegistry,
} from './command-registry';
import { parseShortcut } from './shortcut-keys';

function resetRegistry(): void {
  useCommandRegistry.setState({ commands: {}, order: [], usage: {} });
  window.localStorage.clear();
}

describe('useCommandRegistry', () => {
  beforeEach(() => {
    resetRegistry();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    resetRegistry();
  });

  it('registers and unregisters commands preserving order', () => {
    const first = { id: 'a', title: 'A', run: () => undefined };
    const second = { id: 'b', title: 'B', run: () => undefined };
    const offFirst = useCommandRegistry.getState().register(first);
    useCommandRegistry.getState().register(second);
    expect(useCommandRegistry.getState().order).toEqual(['a', 'b']);

    offFirst();
    expect(useCommandRegistry.getState().order).toEqual(['b']);
    expect(useCommandRegistry.getState().commands['a']).toBeUndefined();
  });

  it('re-registering the same id keeps its original slot and updates the definition', () => {
    const original = { id: 'a', title: 'A', run: () => undefined };
    useCommandRegistry.getState().register(original);
    useCommandRegistry.getState().register({ id: 'b', title: 'B', run: () => undefined });
    const replacement = { id: 'a', title: 'A2', run: () => undefined };
    useCommandRegistry.getState().register(replacement);

    expect(useCommandRegistry.getState().order).toEqual(['a', 'b']);
    expect(useCommandRegistry.getState().commands['a']?.title).toBe('A2');
  });

  it('rejects a conflicting shortcut at registration time naming the loser in dev', () => {
    const winner = { id: 'winner', title: 'Winner', shortcut: 'mod+k', run: () => undefined };
    const loser = { id: 'loser', title: 'Loser', shortcut: 'ctrl+k', run: () => undefined };

    useCommandRegistry.getState().register(winner);
    let thrown: unknown = null;
    try {
      useCommandRegistry.getState().register(loser);
    } catch (error) {
      thrown = error;
    }

    expect(thrown).toBeInstanceOf(Error);
    const message = String((thrown as Error).message);
    expect(message).toContain('"loser"');
    expect(message).toContain('"winner"');
    expect(message).toContain('mod+k');

    // In production-style flow the loser still registers but without binding.
    const registered = useCommandRegistry.getState().commands['loser'];
    if (registered) {
      expect(registered.shortcutRejectedBy).toBe('winner');
    }
  });

  it('treats equivalent-but-differently-written bindings as conflicting', () => {
    useCommandRegistry
      .getState()
      .register({ id: 'one', title: 'One', shortcut: 'ctrl+shift+p', run: () => undefined });
    expect(() =>
      useCommandRegistry
        .getState()
        .register({ id: 'two', title: 'Two', shortcut: 'CTRL+SHIFT+P', run: () => undefined }),
    ).toThrow(/already bound/);
  });

  it('filters availability through predicates', () => {
    useCommandRegistry.getState().register({
      id: 'gated',
      title: 'Gated',
      when: () => false,
      run: () => undefined,
    });
    useCommandRegistry.getState().register({ id: 'open', title: 'Open', run: () => undefined });

    const ids = getAvailableCommands().map((command) => command.id);
    expect(ids).toEqual(['open']);
  });

  it('records runs with recency and persists usage across reloads', () => {
    const ran: string[] = [];
    useCommandRegistry.getState().register({ id: 'x', title: 'X', run: () => ran.push('x') });

    expect(runCommand('missing')).toBe(false);
    expect(runCommand('x')).toBe(true);
    expect(ran).toEqual(['x']);

    const usage = useCommandRegistry.getState().usage;
    expect(usage['x']).toMatchObject({ count: 1 });
    expect(typeof usage['x']?.lastUsedAt).toBe('number');
    expect(window.localStorage.getItem('lazarus.commandUsage.v1')).not.toBeNull();
  });

  it('refuses to run unavailable commands', () => {
    const ran: string[] = [];
    useCommandRegistry.getState().register({
      id: 'gated',
      title: 'Gated',
      when: () => false,
      run: () => ran.push('nope'),
    });
    expect(runCommand('gated')).toBe(false);
    expect(ran).toEqual([]);
  });

  it('resolves shortcut sequences and chord prefixes against live registrations', () => {
    useCommandRegistry
      .getState()
      .register({ id: 'palette', title: 'Palette', shortcut: 'mod+k', run: () => undefined });
    useCommandRegistry
      .getState()
      .register({ id: 'home', title: 'Home', shortcut: 'g h', run: () => undefined });

    expect(findCommandBySequence(parseShortcut('mod+k'))).toBe('palette');
    expect(findCommandBySequence(parseShortcut('g'))).toBeNull();
    expect(hasLongerSequencePrefix(parseShortcut('g'))).toBe(true);
    expect(hasLongerSequencePrefix(parseShortcut('g h'))).toBe(false);
    expect(findCommandBySequence(parseShortcut('g h'))).toBe('home');
  });

  it('keeps usage entries when a command re-registers', () => {
    const definition = { id: 'x', title: 'X', run: () => undefined };
    const off = useCommandRegistry.getState().register(definition);
    runCommand('x');
    off();
    useCommandRegistry.getState().register({ id: 'x', title: 'X', run: () => undefined });
    expect(useCommandRegistry.getState().usage['x']?.count).toBe(1);
  });
});
