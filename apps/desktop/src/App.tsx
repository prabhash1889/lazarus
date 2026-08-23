import { useCallback, useEffect, useState } from 'react';

interface Capability {
  name: string;
  enabled: boolean;
}

interface NegotiatedMethod {
  name: string;
  version: string | null;
  fallback: string | null;
}

interface HostStatus {
  connected: boolean;
  hostVersion: string | null;
  servingStatus: string | null;
  capabilities: Capability[];
  methods: NegotiatedMethod[];
  error: string | null;
}

function methodLabel(method: NegotiatedMethod): string {
  if (method.version) {
    return `${method.name}=${method.version}`;
  }
  if (method.fallback) {
    return `${method.name}=>${method.fallback} (fallback)`;
  }
  return `${method.name}=unavailable`;
}

async function fetchHostStatus(): Promise<HostStatus> {
  const tauri = window.__TAURI__;
  if (!tauri) {
    throw new Error(
      'Tauri bridge unavailable. Launch the app through the desktop shell (pnpm dev:desktop) instead of a plain browser tab.',
    );
  }
  return tauri.core.invoke<HostStatus>('host_status');
}

export function App() {
  const [status, setStatus] = useState<HostStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setStatus(await fetchHostStatus());
    } catch (err) {
      setStatus(null);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <main className="shell">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>
      <section className="host-status" aria-live="polite" aria-busy={loading}>
        {loading ? (
          <p className="muted">Connecting to host at http://127.0.0.1:50051...</p>
        ) : error ? (
          <>
            <p role="alert" className="error">
              {error}
            </p>
            <button type="button" onClick={() => void refresh()}>
              Retry
            </button>
          </>
        ) : status && status.connected ? (
          <>
            <dl className="status-grid">
              <dt>Connection</dt>
              <dd>connected</dd>
              <dt>Host version</dt>
              <dd>{status.hostVersion ?? 'unknown'}</dd>
              <dt>Serving status</dt>
              <dd>{status.servingStatus ?? 'unknown'}</dd>
            </dl>
            {status.methods.length > 0 ? (
              <>
                <h2>Negotiated methods</h2>
                <ul className="capability-list" aria-label="Negotiated protocol methods">
                  {status.methods.map((method) => (
                    <li key={method.name}>
                      <code>{method.name}</code> - {methodLabel(method)}
                    </li>
                  ))}
                </ul>
              </>
            ) : null}
            {status.capabilities.length > 0 ? (
              <>
                <h2>Capabilities</h2>
                <ul className="capability-list">
                  {status.capabilities.map((capability) => (
                    <li key={capability.name}>
                      <code>{capability.name}</code> - {capability.enabled ? 'enabled' : 'disabled'}
                    </li>
                  ))}
                </ul>
              </>
            ) : (
              <p className="muted">No negotiated capabilities.</p>
            )}
            <button type="button" onClick={() => void refresh()}>
              Refresh
            </button>
          </>
        ) : (
          <>
            <p role="alert" className="error">
              {status?.error ?? 'Unknown connection failure.'}
            </p>
            <button type="button" onClick={() => void refresh()}>
              Retry
            </button>
          </>
        )}
      </section>
    </main>
  );
}
