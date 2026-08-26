export interface Keystroke {
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
  /** Lowercase event.key value, with ` ` normalized to `space`. */
  key: string;
}

const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPod|iPhone|iPad/.test(navigator.userAgent);

function normalizeKeyToken(token: string): string {
  const key = token.trim().toLowerCase();
  if (key === '' || key === '+') {
    throw new Error(`Invalid shortcut: empty key in "${token}"`);
  }
  if (key === ' ') {
    return 'space';
  }
  return key;
}

export function parseStroke(stroke: string): Keystroke {
  const parts = stroke.split('+').map((part) => part.trim());
  const key = normalizeKeyToken(parts[parts.length - 1] ?? '');
  const modifiers = parts.slice(0, -1);
  const stroke2: Keystroke = { ctrl: false, alt: false, shift: false, meta: false, key };
  for (const modifier of modifiers) {
    switch (modifier.toLowerCase()) {
      case 'mod':
        if (IS_MAC) {
          stroke2.meta = true;
        } else {
          stroke2.ctrl = true;
        }
        break;
      case 'ctrl':
        stroke2.ctrl = true;
        break;
      case 'alt':
      case 'option':
        stroke2.alt = true;
        break;
      case 'shift':
        stroke2.shift = true;
        break;
      case 'cmd':
      case 'meta':
      case 'command':
        stroke2.meta = true;
        break;
      case '':
        throw new Error(`Invalid shortcut: stray "+" in "${stroke}"`);
      default:
        throw new Error(`Invalid shortcut: unknown modifier "${modifier}" in "${stroke}"`);
    }
  }
  return stroke2;
}

/** Parses a shortcut expression; whitespace separates chord steps. */
export function parseShortcut(combo: string): Keystroke[] {
  const strokes = combo
    .trim()
    .split(/\s+/)
    .filter((part) => part.length > 0)
    .map(parseStroke);
  if (strokes.length === 0) {
    throw new Error('Invalid shortcut: empty binding');
  }
  return strokes;
}

export function keystrokeFromEvent(event: {
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  key: string;
}): Keystroke {
  return {
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    meta: event.metaKey,
    key: normalizeKeyToken(event.key === ' ' ? 'space' : event.key),
  };
}

export function signatureOfStroke(stroke: Keystroke): string {
  return [
    stroke.ctrl ? 'ctrl+' : '',
    stroke.alt ? 'alt+' : '',
    stroke.shift ? 'shift+' : '',
    stroke.meta ? 'meta+' : '',
    stroke.key,
  ].join('');
}

export function signatureOfSequence(sequence: Keystroke[]): string {
  return sequence.map(signatureOfStroke).join(' ');
}

function matchesStroke(eventStroke: Keystroke, target: Keystroke): boolean {
  if (eventStroke.key !== target.key) {
    return false;
  }
  // The key already reflects Shift for printable characters, so an explicit
  // requirement is honored but a bare binding does not demand shiftlessness.
  if (target.shift && !eventStroke.shift) {
    return false;
  }
  return (
    eventStroke.ctrl === target.ctrl &&
    eventStroke.alt === target.alt &&
    eventStroke.meta === target.meta
  );
}

export function sequenceMatches(eventStrokes: Keystroke[], target: Keystroke[]): boolean {
  if (eventStrokes.length !== target.length) {
    return false;
  }
  return eventStrokes.every((stroke, index) => {
    const targetStroke = target[index];
    return targetStroke !== undefined && matchesStroke(stroke, targetStroke);
  });
}

export function isPrefixOf(target: Keystroke[], prefix: Keystroke[]): boolean {
  if (prefix.length >= target.length) {
    return false;
  }
  return prefix.every((stroke, index) => {
    const targetStroke = target[index];
    return targetStroke !== undefined && matchesStroke(stroke, targetStroke);
  });
}

const DISPLAY_KEY: Record<string, string> = {
  space: 'Space',
  escape: 'Esc',
  arrowup: 'Up',
  arrowdown: 'Down',
  arrowleft: 'Left',
  arrowright: 'Right',
  enter: 'Enter',
  tab: 'Tab',
  backspace: 'Backspace',
  delete: 'Delete',
  home: 'Home',
  end: 'End',
  pageup: 'PageUp',
  pagedown: 'PageDown',
};

function displayStroke(stroke: Keystroke): string {
  const parts: string[] = [];
  if (stroke.ctrl) {
    parts.push(IS_MAC ? 'Ctrl' : 'Ctrl');
  }
  if (stroke.alt) {
    parts.push(IS_MAC ? 'Option' : 'Alt');
  }
  if (stroke.shift) {
    parts.push('Shift');
  }
  if (stroke.meta) {
    parts.push(IS_MAC ? 'Cmd' : 'Meta');
  }
  const key =
    stroke.key.length === 1 ? stroke.key.toUpperCase() : (DISPLAY_KEY[stroke.key] ?? stroke.key);
  parts.push(key);
  return parts.join('+');
}

/** Formats a parsed shortcut for UI display, e.g. `Ctrl+K` or `G then H`. */
export function formatShortcut(strokes: Keystroke[]): string {
  return strokes.map(displayStroke).join(' then ');
}
