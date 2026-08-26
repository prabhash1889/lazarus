import { useEffect, type RefObject } from 'react';

/**
 * Focus containment and restoration primitives (Phase 3.5). Radix-based
 * dialogs and the command palette already trap focus via their primitives;
 * these helpers cover Lazarus-owned surfaces such as menus, drawers, and
 * future custom modals so keyboard focus never escapes an active overlay
 * and always returns to its origin when the overlay closes.
 */

const FOCUS_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(', ');

/** All currently tabbable elements inside `container`, in DOM order. */
export function getTabbables(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUS_SELECTOR)).filter(
    (element) => element.tabIndex >= 0 && !element.hasAttribute('disabled'),
  );
}

/**
 * Contains Tab/Shift+Tab cycling inside `container`. Returns a release
 * function that removes the listener. Focus itself is not moved.
 */
export function trapFocus(container: HTMLElement): () => void {
  const onKeyDown = (event: KeyboardEvent): void => {
    if (event.key !== 'Tab') {
      return;
    }
    const tabbables = getTabbables(container);
    if (tabbables.length === 0) {
      event.preventDefault();
      return;
    }
    const first = tabbables[0]!;
    const last = tabbables[tabbables.length - 1]!;
    const active = document.activeElement;
    if (event.shiftKey) {
      if (active === first || !container.contains(active)) {
        event.preventDefault();
        last.focus();
      }
    } else if (active === last || !container.contains(active)) {
      event.preventDefault();
      first.focus();
    }
  };
  document.addEventListener('keydown', onKeyDown, true);
  return () => document.removeEventListener('keydown', onKeyDown, true);
}

export interface UseFocusTrapOptions {
  /** Whether the trap is currently engaged (e.g. a menu is open). */
  active: boolean;
  containerRef: RefObject<HTMLElement | null>;
  /**
   * Moves focus here when activated; defaults to the first tabbable
   * descendant, then the container itself.
   */
  initialFocus?: () => HTMLElement | null;
}

/**
 * Traps focus inside `containerRef` while `active`, focuses the initial
 * element, and restores focus to the previously focused element when the
 * trap releases. Restores synchronously on deactivate so callers can close
 * and hand focus back in one render cycle.
 */
export function useFocusTrap(options: UseFocusTrapOptions): void {
  const { active, containerRef, initialFocus } = options;

  useEffect(() => {
    if (!active) {
      return undefined;
    }
    const container = containerRef.current;
    if (container === null) {
      return undefined;
    }
    const previouslyFocused = document.activeElement;
    const target =
      initialFocus?.() ??
      getTabbables(container)[0] ??
      (container.matches(FOCUS_SELECTOR) ? container : null);
    target?.focus();
    const release = trapFocus(container);
    return () => {
      release();
      if (previouslyFocused instanceof HTMLElement && previouslyFocused.isConnected) {
        previouslyFocused.focus();
      }
    };
  }, [active, containerRef, initialFocus]);
}
