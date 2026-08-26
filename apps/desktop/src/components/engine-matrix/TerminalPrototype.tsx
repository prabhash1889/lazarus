import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Terminal } from '@xterm/xterm';

import '@xterm/xterm/css/xterm.css';

/**
 * Phase 3.5 prototype of the terminal-agent tile surface. Mounts a real
 * xterm.js Terminal so the per-engine matrix can validate PTY-style
 * rendering and keyboard input on WebView2, WKWebView, and WebKitGTK.
 * The PTY bridge itself arrives with provider adapters (Phase 6+); typed
 * input is echoed locally here.
 */
export function TerminalPrototype(): ReactNode {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [status, setStatus] = useState<'pending' | 'ready' | 'failed'>('pending');
  const [failure, setFailure] = useState<string | null>(null);
  const [echoed, setEchoed] = useState<string[]>([]);

  useEffect(() => {
    const element = containerRef.current;
    if (element === null || element.childElementCount > 0) {
      return;
    }
    let disposed = false;
    let term: Terminal | null = null;
    try {
      term = new Terminal({
        convertEol: true,
        fontSize: 12,
        allowProposedApi: true,
      });
      term.open(element);
      term.writeln('Lazarus engine matrix - xterm.js prototype');
      term.writeln('Type to verify input handling on this engine.');
      term.onData((data) => {
        if (disposed) {
          return;
        }
        // Record echoes for programmatic verification; render Enter as CRLF.
        setEchoed((previous) => [...previous.slice(-49), data]);
        term?.write(data === '\r' ? '\r\n' : data);
      });
      setStatus('ready');
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
      setStatus('failed');
    }
    return () => {
      disposed = true;
      try {
        term?.dispose();
      } catch {
        // jsdom teardown can race renderer disposal; nothing to recover.
      }
    };
  }, []);

  return (
    <div className="engine-proto" data-testid="terminal-prototype" data-status={status}>
      <p className="muted engine-proto-note">
        xterm.js status: <strong>{status}</strong>
        {echoed.length > 0 ? ` - ${echoed.length} input events` : ''}
      </p>
      {failure !== null ? (
        <p role="alert" className="error">
          {failure}
        </p>
      ) : null}
      <div ref={containerRef} className="terminal-proto-view" data-testid="terminal-view" />
    </div>
  );
}
