import { act } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { pushToast, useToastStore } from './toast-store';

describe('toast-store', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useToastStore.setState({ toasts: [] });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('pushes toasts and dismisses them by id', () => {
    pushToast({ kind: 'info', title: 'hello' });
    expect(useToastStore.getState().toasts).toHaveLength(1);
    const [toast] = useToastStore.getState().toasts;
    if (!toast) {
      throw new Error('expected a toast');
    }
    useToastStore.getState().dismiss(toast.id);
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('auto-dismisses after the timeout', () => {
    pushToast({ kind: 'error', title: 'boom' });
    expect(useToastStore.getState().toasts).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it('caps the visible stack', () => {
    for (let i = 0; i < 8; i++) {
      pushToast({ kind: 'info', title: `toast-${i}` });
    }
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(5);
    expect(toasts[0]?.title).toBe('toast-3');
  });
});
