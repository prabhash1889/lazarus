import { describe, expect, it } from 'vitest';

import {
  formatShortcut,
  isPrefixOf,
  keystrokeFromEvent,
  parseShortcut,
  sequenceMatches,
  signatureOfSequence,
} from './shortcut-keys';

describe('parseShortcut', () => {
  it('parses single keystrokes with modifiers', () => {
    const strokes = parseShortcut('ctrl+shift+p');
    expect(strokes).toHaveLength(1);
    expect(strokes[0]).toMatchObject({ ctrl: true, shift: true, key: 'p' });
  });

  it('parses chord sequences separated by whitespace', () => {
    const strokes = parseShortcut('g h');
    expect(strokes).toHaveLength(2);
    expect(strokes.map((stroke) => stroke.key)).toEqual(['g', 'h']);
  });

  it('normalizes the space key', () => {
    const strokes = parseShortcut('ctrl+space');
    expect(strokes[0]?.key).toBe('space');
  });

  it('rejects empty and malformed bindings', () => {
    expect(() => parseShortcut('')).toThrow();
    expect(() => parseShortcut('ctrl+')).toThrow();
    expect(() => parseShortcut('++k')).toThrow();
    expect(() => parseShortcut('ctrl+bogus+k')).toThrow();
  });
});

describe('keystrokeFromEvent + sequenceMatches', () => {
  function fakeEvent(overrides: Partial<KeyboardEvent>): KeyboardEvent {
    return {
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
      metaKey: false,
      key: '',
      ...overrides,
    } as KeyboardEvent;
  }

  it('matches plain keys regardless of incidental shift', () => {
    const binding = parseShortcut('g');
    const event = keystrokeFromEvent(fakeEvent({ key: 'g', shiftKey: true }));
    expect(sequenceMatches([event], binding)).toBe(true);
  });

  it('honors explicit shift requirements', () => {
    const binding = parseShortcut('shift+s');
    const withoutShift = keystrokeFromEvent(fakeEvent({ key: 's' }));
    const withShift = keystrokeFromEvent(fakeEvent({ key: 'S', shiftKey: true }));
    expect(sequenceMatches([withoutShift], binding)).toBe(false);
    expect(sequenceMatches([withShift], binding)).toBe(true);
  });

  it('requires exact ctrl/alt/meta modifiers', () => {
    const binding = parseShortcut('mod+k');
    // On this test platform (non-Mac) mod resolves to ctrl.
    const ctrlK = keystrokeFromEvent(fakeEvent({ key: 'k', ctrlKey: true }));
    const metaK = keystrokeFromEvent(fakeEvent({ key: 'k', metaKey: true }));
    const plainK = keystrokeFromEvent(fakeEvent({ key: 'k' }));
    expect(sequenceMatches([ctrlK], binding)).toBe(true);
    expect(sequenceMatches([metaK], binding)).toBe(false);
    expect(sequenceMatches([plainK], binding)).toBe(false);
  });

  it('matches full chords in order', () => {
    const binding = parseShortcut('g h');
    const strokes = [
      keystrokeFromEvent(fakeEvent({ key: 'g' })),
      keystrokeFromEvent(fakeEvent({ key: 'h' })),
    ];
    expect(sequenceMatches(strokes, binding)).toBe(true);
    expect(isPrefixOf(binding, strokes.slice(0, 1))).toBe(true);
    expect(isPrefixOf(parseShortcut('g'), strokes)).toBe(false);
  });
});

describe('formatShortcut', () => {
  it('formats single keystrokes', () => {
    expect(formatShortcut(parseShortcut('mod+k'))).toContain('+K');
  });

  it('formats chords with an explicit separator', () => {
    const formatted = formatShortcut(parseShortcut('g h'));
    expect(formatted).toBe('G then H');
  });
});

describe('signatureOfSequence', () => {
  it('is canonical for equivalent bindings', () => {
    expect(signatureOfSequence(parseShortcut('ctrl+k'))).toBe(
      signatureOfSequence(parseShortcut('CTRL+K')),
    );
    expect(signatureOfSequence(parseShortcut('g h'))).not.toBe(
      signatureOfSequence(parseShortcut('h g')),
    );
  });
});
