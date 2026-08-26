import { describe, expect, it } from 'vitest';

import { ShortcutEngine, defaultShouldIgnore, strokesForBinding } from './shortcut-engine';

function keyEvent(key: string, overrides: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return {
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    key,
    target: { tagName: 'BODY' },
    ...overrides,
  } as unknown as KeyboardEvent;
}

interface EngineHarness {
  engine: ShortcutEngine;
  fired: string[];
}

function makeHarness(bindings: Record<string, string>, options = {}): EngineHarness {
  const fired: string[] = [];
  const entries = Object.entries(bindings);
  const engine = new ShortcutEngine(
    {
      lookup: (sequence) => {
        for (const [id, binding] of entries) {
          const strokes = strokesForBinding(binding);
          if (
            strokes.length === sequence.length &&
            strokes.every(
              (stroke, index) =>
                stroke.key === sequence[index]?.key &&
                stroke.ctrl === sequence[index]?.ctrl &&
                stroke.alt === sequence[index]?.alt &&
                stroke.meta === sequence[index]?.meta &&
                (stroke.shift ? sequence[index]?.shift : true),
            )
          ) {
            return id;
          }
        }
        return null;
      },
      hasExtension: (sequence) =>
        entries.some(([, binding]) => {
          const strokes = strokesForBinding(binding);
          return (
            strokes.length > sequence.length &&
            sequence.every(
              (stroke, index) =>
                strokes[index]?.key === stroke.key &&
                strokes[index]?.ctrl === stroke.ctrl &&
                strokes[index]?.alt === stroke.alt &&
                strokes[index]?.meta === stroke.meta,
            )
          );
        }),
      execute: (id) => {
        fired.push(id);
        return true;
      },
      shouldIgnore: defaultShouldIgnore,
    },
    options,
  );
  return { engine, fired };
}

describe('ShortcutEngine', () => {
  it('fires single-key shortcuts immediately', () => {
    const { engine, fired } = makeHarness({ palette: 'mod+k' });
    const consumed = engine.handleKeyDown(keyEvent('k', { ctrlKey: true }));
    expect(consumed).toBe(true);
    expect(fired).toEqual(['palette']);
  });

  it('buffers chord prefixes and fires when the chord completes', () => {
    const { engine, fired } = makeHarness({ home: 'g h' });

    const first = engine.handleKeyDown(keyEvent('g'));
    expect(first).toBe(true);
    expect(fired).toEqual([]);

    const second = engine.handleKeyDown(keyEvent('h'));
    expect(second).toBe(true);
    expect(fired).toEqual(['home']);
  });

  it('ignores unrelated keys typed mid-chord and resets the buffer', () => {
    const { engine, fired } = makeHarness({ home: 'g h', settings: 'g s' });
    engine.handleKeyDown(keyEvent('g'));
    const consumed = engine.handleKeyDown(keyEvent('x'));
    expect(consumed).toBe(false);
    expect(fired).toEqual([]);

    engine.handleKeyDown(keyEvent('g'));
    engine.handleKeyDown(keyEvent('s'));
    expect(fired).toEqual(['settings']);
  });

  it('expires stale chord buffers after the timeout', () => {
    let nowMs = 0;
    const { engine, fired } = makeHarness({ home: 'g h' }, { now: () => nowMs });

    engine.handleKeyDown(keyEvent('g'));
    nowMs = 5000;
    engine.handleKeyDown(keyEvent('h'));
    expect(fired).toEqual([]);
  });

  it('leaves unmatched events unconsumed', () => {
    const { engine, fired } = makeHarness({ home: 'g h' });
    expect(engine.handleKeyDown(keyEvent('z'))).toBe(false);
    expect(fired).toEqual([]);
  });

  it('skips editable targets unless a modifier is held', () => {
    const { engine, fired } = makeHarness({ palette: 'mod+k', plain: 'a' });
    const typingTarget = document.createElement('input');
    document.body.appendChild(typingTarget);
    try {
      expect(
        defaultShouldIgnore(keyEvent('a', { target: typingTarget }), {
          ctrl: false,
          alt: false,
          shift: false,
          meta: false,
          key: 'a',
        }),
      ).toBe(true);
      expect(
        defaultShouldIgnore(keyEvent('k', { target: typingTarget, ctrlKey: true }), {
          ctrl: true,
          alt: false,
          shift: false,
          meta: false,
          key: 'k',
        }),
      ).toBe(false);

      engine.handleKeyDown(keyEvent('a', { target: typingTarget }));
      expect(fired).toEqual([]);
      engine.handleKeyDown(keyEvent('k', { target: typingTarget, ctrlKey: true }));
      expect(fired).toEqual(['palette']);
    } finally {
      typingTarget.remove();
    }
  });
});
