import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';

import { DiffPrototype } from './DiffPrototype';
import { EditorPrototype } from './EditorPrototype';
import { TerminalPrototype } from './TerminalPrototype';

/**
 * The engine prototypes must mount and accept input in the test
 * environment; per-engine (WebView2/WKWebView/WebKitGTK) results are then
 * recorded manually or via packaged CI builds into the tracking doc.
 */

afterEach(() => {
  cleanup();
});

describe('diff prototype', () => {
  it('renders hunks with signed, colored lines', () => {
    const diffText = [
      '--- a/f.ts',
      '+++ b/f.ts',
      '@@ -1,2 +1,2 @@',
      '-gone',
      '+here',
      ' keep',
    ].join('\n');
    render(<DiffPrototype diffText={diffText} />);

    const view = screen.getByTestId('diff-prototype');
    const add = view.querySelector('.diff-line-add');
    const remove = view.querySelector('.diff-line-remove');
    expect(add?.textContent).toContain('here');
    expect(remove?.textContent).toContain('gone');
    expect(view.querySelectorAll('.diff-line-context')).toHaveLength(1);
  });
});

describe('terminal prototype', () => {
  it('mounts the xterm view and reports a definite status', async () => {
    render(<TerminalPrototype />);
    const proto = screen.getByTestId('terminal-prototype');
    // xterm either initializes (ready) or reports a typed failure; a stuck
    // "pending" state is the only unacceptable outcome.
    await vi.waitFor(() => {
      expect(['ready', 'failed']).toContain(proto.getAttribute('data-status'));
    });
    expect(screen.getByTestId('terminal-view')).toBeTruthy();
  });
});

describe('editor prototype', () => {
  function Harness(): ReactNode {
    return (
      <main>
        <h1>Editor</h1>
        <EditorPrototype />
      </main>
    );
  }

  it('mounts CodeMirror and accepts typed input', async () => {
    const user = userEvent.setup();
    const { container } = render(<Harness />);

    await vi.waitFor(() => {
      expect(['ready', 'failed']).toContain(
        screen.getByTestId('editor-prototype').getAttribute('data-status'),
      );
    });
    expect(screen.getByTestId('editor-prototype').getAttribute('data-status')).toBe('ready');

    const content = container.querySelector('.cm-content');
    expect(content).toBeTruthy();
    content!.dispatchEvent(new FocusEvent('focus'));
    await user.type(content as HTMLElement, 'X');

    const note = screen.getByTestId('editor-prototype').querySelector('.engine-proto-note');
    expect(note?.textContent).toContain('document updates');
  });
});
