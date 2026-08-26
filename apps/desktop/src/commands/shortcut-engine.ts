import { keystrokeFromEvent, parseShortcut, type Keystroke } from './shortcut-keys';

const DEFAULT_CHORD_TIMEOUT_MS = 1200;

export interface ShortcutEngineOptions {
  chordTimeoutMs?: number;
  now?: () => number;
}

export interface ShortcutEngineHooks {
  /** Resolves a pending sequence to a command id, or null when unmatched. */
  lookup: (sequence: Keystroke[]) => string | null;
  /** True when some binding extends `sequence` as a proper prefix. */
  hasExtension: (sequence: Keystroke[]) => boolean;
  /** Executes a resolved command; returns false when unavailable. */
  execute: (id: string) => boolean;
  /** Allows callers to skip events (e.g. typing in editable targets). */
  shouldIgnore?: (event: KeyboardEvent, firstStroke: Keystroke) => boolean;
}

/**
 * Dispatches keydown events against registered shortcuts, buffering chord
 * prefixes such as `g h` until the chord completes or times out.
 */
export class ShortcutEngine {
  private pending: Keystroke[] = [];
  private pendingAt = 0;
  private readonly chordTimeoutMs: number;
  private readonly now: () => number;

  constructor(
    private readonly hooks: ShortcutEngineHooks,
    options: ShortcutEngineOptions = {},
  ) {
    this.chordTimeoutMs = options.chordTimeoutMs ?? DEFAULT_CHORD_TIMEOUT_MS;
    this.now = options.now ?? (() => Date.now());
  }

  reset(): void {
    this.pending = [];
    this.pendingAt = 0;
  }

  /**
   * Returns true when the engine consumed the event (matched a command or
   * buffered a chord step), meaning callers should preventDefault.
   */
  handleKeyDown(event: KeyboardEvent): boolean {
    const stroke = keystrokeFromEvent(event);
    if (this.hooks.shouldIgnore?.(event, stroke)) {
      return false;
    }
    const timestamp = this.now();
    if (this.pending.length > 0 && timestamp - this.pendingAt > this.chordTimeoutMs) {
      this.reset();
    }
    const sequence = [...this.pending, stroke];
    const exactId = this.hooks.lookup(sequence);
    const canExtend = this.hooks.hasExtension(sequence);
    if (exactId === null && !canExtend) {
      this.reset();
      return false;
    }
    if (exactId !== null && !canExtend) {
      this.reset();
      return this.hooks.execute(exactId);
    }
    // Either only a prefix match or an ambiguous exact+extension pair; keep
    // buffering so longer chords win.
    this.pending = sequence;
    this.pendingAt = timestamp;
    return true;
  }
}

/** Default ignore rule: unmodified keys typed in editable targets. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.isContentEditable) {
    return true;
  }
  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
}

export function defaultShouldIgnore(event: KeyboardEvent, stroke: Keystroke): boolean {
  if (!isEditableTarget(event.target)) {
    return false;
  }
  return !(stroke.ctrl || stroke.alt || stroke.meta);
}

export function strokesForBinding(binding: string): Keystroke[] {
  return parseShortcut(binding);
}
