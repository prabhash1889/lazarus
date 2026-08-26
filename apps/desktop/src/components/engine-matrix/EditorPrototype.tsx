import { useEffect, useRef, useState, type ReactNode } from 'react';
import { EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { indentOnInput } from '@codemirror/language';

/**
 * Phase 3.5 prototype of the artifact/file editor surface. Mounts a real
 * CodeMirror 6 view so the per-engine matrix can validate measurement,
 * IME/input handling, and scrolling on all three WebView engines. The
 * artifact editor integration arrives with the artifact system (Phase 7+).
 */

export function EditorPrototype(): ReactNode {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [status, setStatus] = useState<'pending' | 'ready' | 'failed'>('pending');
  const [failure, setFailure] = useState<string | null>(null);
  const [changeCount, setChangeCount] = useState(0);

  useEffect(() => {
    const element = containerRef.current;
    if (element === null || element.childElementCount > 0) {
      return;
    }
    let disposed = false;
    let view: EditorView | null = null;
    try {
      view = new EditorView({
        parent: element,
        state: EditorState.create({
          doc: 'The artifact editor will live here.\nEdit this text to verify input on this engine.',
          extensions: [
            history(),
            indentOnInput(),
            keymap.of(defaultKeymap),
            keymap.of(historyKeymap),
            EditorView.contentAttributes.of({ 'aria-label': 'Artifact editor prototype' }),
            EditorView.updateListener.of((update) => {
              if (!disposed && update.docChanged) {
                setChangeCount((count) => count + 1);
              }
            }),
          ],
        }),
      });
      setStatus('ready');
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
      setStatus('failed');
    }
    return () => {
      disposed = true;
      view?.destroy();
    };
  }, []);

  return (
    <div className="engine-proto" data-testid="editor-prototype" data-status={status}>
      <p className="muted engine-proto-note">
        CodeMirror 6 status: <strong>{status}</strong>
        {changeCount > 0 ? ` - ${changeCount} document updates` : ''}
      </p>
      {failure !== null ? (
        <p role="alert" className="error">
          {failure}
        </p>
      ) : null}
      <div ref={containerRef} className="editor-proto-view" data-testid="editor-view" />
    </div>
  );
}
