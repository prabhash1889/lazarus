import { act } from 'react';
import { renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { useReducedMotion } from './use-reduced-motion';

type Listener = (event: { matches: boolean }) => void;

function stubMatchMedia(initial: boolean): {
  listeners: Set<Listener>;
  set: (matches: boolean) => void;
} {
  const listeners = new Set<Listener>();
  let matches = initial;
  vi.stubGlobal(
    'matchMedia',
    vi.fn().mockImplementation((query: string) => ({
      matches,
      media: query,
      addEventListener: (_: string, listener: Listener) => listeners.add(listener),
      removeEventListener: (_: string, listener: Listener) => listeners.delete(listener),
    })),
  );
  return {
    listeners,
    set(next: boolean): void {
      matches = next;
      act(() => {
        for (const listener of listeners) {
          listener({ matches: next });
        }
      });
    },
  };
}

describe('useReducedMotion', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('reports the OS preference and live-updates when it changes', () => {
    const media = stubMatchMedia(false);
    const { result } = renderHook(() => useReducedMotion());
    expect(result.current).toBe(false);

    media.set(true);
    expect(result.current).toBe(true);

    media.set(false);
    expect(result.current).toBe(false);
  });

  it('starts reduced when the OS already prefers reduced motion', () => {
    stubMatchMedia(true);
    const { result } = renderHook(() => useReducedMotion());
    expect(result.current).toBe(true);
  });

  it('defaults to full motion where matchMedia is unavailable', () => {
    vi.stubGlobal('matchMedia', undefined);
    const { result } = renderHook(() => useReducedMotion());
    expect(result.current).toBe(false);
  });
});
