import { create } from 'zustand';

import { pushToast } from '../state/toast-store';
import { parseShortcut, sequenceMatches, signatureOfSequence } from './shortcut-keys';
import type { RegisteredCommand, CommandDefinition } from './types';
import type { UsageMap } from './search';

const USAGE_STORAGE_KEY = 'lazarus.commandUsage.v1';

const IS_DEV: boolean =
  typeof import.meta !== 'undefined' &&
  typeof import.meta.env !== 'undefined' &&
  import.meta.env.DEV === true;

function loadUsage(): UsageMap {
  if (typeof window === 'undefined' || !window.localStorage) {
    return {};
  }
  try {
    const raw = window.localStorage.getItem(USAGE_STORAGE_KEY);
    if (!raw) {
      return {};
    }
    const parsed = JSON.parse(raw) as UsageMap;
    return typeof parsed === 'object' && parsed !== null ? parsed : {};
  } catch {
    return {};
  }
}

function persistUsage(usage: UsageMap): void {
  if (typeof window === 'undefined' || !window.localStorage) {
    return;
  }
  try {
    window.localStorage.setItem(USAGE_STORAGE_KEY, JSON.stringify(usage));
  } catch {
    // Persistence is an optimization; in-memory recency still applies.
  }
}

interface CommandRegistryState {
  commands: Record<string, RegisteredCommand>;
  /** Registration order for stable empty-query ranking. */
  order: string[];
  usage: UsageMap;
  register: (definition: CommandDefinition) => () => void;
  unregister: (id: string, definition: CommandDefinition) => void;
  recordRun: (id: string) => void;
}

function rejectConflictingShortcut(
  existing: RegisteredCommand | undefined,
  incoming: CommandDefinition,
): boolean {
  if (!existing || !existing.shortcut || !incoming.shortcut) {
    return false;
  }
  let incomingStrokes;
  try {
    incomingStrokes = parseShortcut(incoming.shortcut);
  } catch (error) {
    announceShortcutProblem(incoming, error instanceof Error ? error.message : String(error));
    return true;
  }
  const existingStrokes = parseShortcut(existing.shortcut);
  if (!sequenceMatches(incomingStrokes, existingStrokes)) {
    return false;
  }
  const message =
    `Shortcut "${incoming.shortcut}" requested by command "${incoming.id}" conflicts with ` +
    `"${existing.shortcut}" already bound to command "${existing.id}" (${existing.title}). ` +
    'The later registration loses its binding.';
  if (IS_DEV) {
    throw new Error(message);
  }
  console.error(`[commands] ${message}`);
  pushToast({ kind: 'error', title: 'Shortcut conflict', detail: message });
  return true;
}

function announceShortcutProblem(definition: CommandDefinition, detail: string): void {
  const message = `Command "${definition.id}" has an invalid shortcut binding: ${detail}`;
  if (IS_DEV) {
    throw new Error(message);
  }
  console.error(`[commands] ${message}`);
}

export const useCommandRegistry = create<CommandRegistryState>((set, get) => ({
  commands: {},
  order: [],
  usage: loadUsage(),
  register: (definition) => {
    const state = get();
    const current = state.commands[definition.id];
    let shortcutRejectedBy: string | undefined;
    if (definition.shortcut) {
      const conflict = Object.values(state.commands).find(
        (candidate) =>
          candidate.id !== definition.id &&
          candidate.shortcut !== undefined &&
          sharesBinding(candidate.shortcut, definition.shortcut ?? ''),
      );
      if (conflict) {
        rejectConflictingShortcut(conflict, definition);
        shortcutRejectedBy = conflict.id;
      } else {
        try {
          parseShortcut(definition.shortcut);
        } catch (error) {
          announceShortcutProblem(
            definition,
            error instanceof Error ? error.message : String(error),
          );
        }
      }
    }
    const registered: RegisteredCommand = { ...definition, shortcutRejectedBy };
    set((state2) => ({
      commands: { ...state2.commands, [definition.id]: registered },
      order: current ? state2.order : [...state2.order, definition.id],
      usage: state2.usage,
    }));
    return () => get().unregister(definition.id, registered);
  },
  unregister: (id, definition) => {
    set((state) => {
      if (state.commands[id] !== definition) {
        return state;
      }
      const commands = { ...state.commands };
      delete commands[id];
      return { commands, order: state.order.filter((orderedId) => orderedId !== id) };
    });
  },
  recordRun: (id) => {
    set((state) => {
      const previous = state.usage[id];
      const usage: UsageMap = {
        ...state.usage,
        [id]: { count: (previous?.count ?? 0) + 1, lastUsedAt: Date.now() },
      };
      persistUsage(usage);
      return { usage };
    });
  },
}));

function sharesBinding(a: string, b: string): boolean {
  try {
    return signatureOfSequence(parseShortcut(a)) === signatureOfSequence(parseShortcut(b));
  } catch {
    return false;
  }
}

/** All currently available commands in registration order. */
export function getAvailableCommands(): RegisteredCommand[] {
  const state = useCommandRegistry.getState();
  return state.order
    .map((id) => state.commands[id])
    .filter((command): command is RegisteredCommand => Boolean(command))
    .filter((command) => !command.when || command.when());
}

export function findCommandBySequence(strokes: ReturnType<typeof parseShortcut>): string | null {
  const targetSignature = signatureOfSequence(strokes);
  for (const command of getAvailableCommands()) {
    if (!command.shortcut) {
      continue;
    }
    try {
      if (signatureOfSequence(parseShortcut(command.shortcut)) === targetSignature) {
        return command.id;
      }
    } catch {
      continue;
    }
  }
  return null;
}

export function hasLongerSequencePrefix(prefix: ReturnType<typeof parseShortcut>): boolean {
  for (const command of getAvailableCommands()) {
    if (!command.shortcut) {
      continue;
    }
    try {
      const strokes = parseShortcut(command.shortcut);
      if (
        strokes.length > prefix.length &&
        signatureOfSequence(strokes.slice(0, prefix.length)) === signatureOfSequence(prefix)
      ) {
        return true;
      }
    } catch {
      continue;
    }
  }
  return false;
}

/** Runs a command by id if it exists and is available; records recency. */
export function runCommand(id: string): boolean {
  const state = useCommandRegistry.getState();
  const command = state.commands[id];
  if (!command || (command.when && !command.when())) {
    return false;
  }
  command.run();
  state.recordRun(id);
  return true;
}
