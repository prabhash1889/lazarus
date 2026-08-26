import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useRef, useState, type ReactNode } from 'react';
import { afterEach, describe, expect, it } from 'vitest';

import { getTabbables, trapFocus, useFocusTrap } from './focus-trap';

describe('getTabbables', () => {
  it('lists enabled tabbables in DOM order and skips disabled/negative entries', () => {
    const container = document.createElement('div');
    container.innerHTML = [
      '<button id="a">a</button>',
      '<button id="b" disabled>b</button>',
      '<input id="c" />',
      '<span id="d" tabindex="-1">d</span>',
      '<a id="e" href="#e">e</a>',
      '<div id="f" tabindex="0">f</div>',
    ].join('');
    document.body.append(container);
    try {
      expect(getTabbables(container).map((el) => el.id)).toEqual(['a', 'c', 'e', 'f']);
    } finally {
      container.remove();
    }
  });
});

describe('trapFocus', () => {
  it('cycles Tab at the boundaries and contains focus inside the container', async () => {
    const user = userEvent.setup();
    function Overlay(): ReactNode {
      return (
        <div data-testid="overlay">
          <button>first</button>
          <input aria-label="middle" />
          <button>last</button>
        </div>
      );
    }
    const { unmount } = render(<Overlay />);
    const overlay = screen.getByTestId('overlay');
    const release = trapFocus(overlay);

    try {
      const [first, last] = overlay.querySelectorAll('button');
      first!.focus();

      // Shift+Tab from the first element wraps to the last.
      await user.keyboard('{Shift>}{Tab}{/Shift}');
      expect(document.activeElement).toBe(last);

      // Tab from the last element wraps back to the first.
      await user.keyboard('{Tab}');
      expect(document.activeElement).toBe(first);

      // Focus that escapes (e.g. programmatically) is pulled back in.
      (document.body as HTMLElement).focus();
      await user.keyboard('{Tab}');
      expect(overlay.contains(document.activeElement)).toBe(true);
    } finally {
      release();
      unmount();
    }
  });

  it('keeps Tab a no-op when nothing inside is tabbable', () => {
    const container = document.createElement('div');
    container.innerHTML = '<p>static only</p>';
    document.body.append(container);
    const release = trapFocus(container);
    try {
      const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
      document.body.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    } finally {
      release();
      container.remove();
    }
  });
});

describe('useFocusTrap', () => {
  afterEach(() => {
    cleanup();
  });

  function Harness({ open }: { open: boolean }): ReactNode {
    const menuRef = useRef<HTMLDivElement | null>(null);
    useFocusTrap({ active: open, containerRef: menuRef });
    return (
      <>
        <button data-testid="opener">opener</button>
        {open ? (
          <div ref={menuRef} data-testid="menu">
            <button data-testid="item-1">one</button>
            <button data-testid="item-2">two</button>
          </div>
        ) : null}
      </>
    );
  }

  function Toggling(): ReactNode {
    const [open, setOpen] = useState(false);
    return (
      <>
        <button data-testid="toggle" onClick={() => setOpen((value) => !value)}>
          toggle
        </button>
        <Harness open={open} />
      </>
    );
  }

  it('focuses the first tabbable on activation and restores the origin on close', async () => {
    const user = userEvent.setup();
    render(<Toggling />);

    await user.click(screen.getByTestId('toggle'));
    expect(document.activeElement).toBe(screen.getByTestId('item-1'));

    await user.click(screen.getByTestId('toggle'));
    expect(document.activeElement).toBe(screen.getByTestId('toggle'));
  });

  it('releases containment when deactivated so outside buttons are reachable', async () => {
    const user = userEvent.setup();
    render(<Toggling />);
    await user.click(screen.getByTestId('toggle'));
    await user.click(screen.getByTestId('toggle'));
    await user.click(screen.getByTestId('toggle'));
    // Menu reopened: trap engaged again around the items.
    expect(document.activeElement).toBe(screen.getByTestId('item-1'));

    // Close via state once more; afterwards the opener stays clickable
    // without the trap intercepting Tab presses.
    await user.click(screen.getByTestId('toggle'));
    const before = document.activeElement;
    await user.keyboard('{Tab}');
    expect(document.activeElement).not.toBe(before);
  });
});
