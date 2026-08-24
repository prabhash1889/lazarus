import { useCallback, useEffect, useRef, useState } from 'react';

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

interface ActionResult {
  ok: boolean;
  detail?: string | null;
  error?: string | null;
}

interface DoctorResult {
  ok: boolean;
  report?: unknown;
  error?: string | null;
}

type Transition = 'idle' | 'starting' | 'stopping';

type ConnectionHistory = 'never-up' | 'up' | 'down';

const POLL_INTERVAL_MS = 3000;

function methodLabel(method: NegotiatedMethod): string {
  if (method.version) {
    return `${method.name}=${method.version}`;
  }
  if (method.fallback) {
    return `${method.name}=>${method.fallback} (fallback)`;
  }
  return `${method.name}=unavailable`;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function invokeCommand<T>(cmd: string): Promise<T> {
  const tauri = window.__TAURI__;
  if (!tauri) {
    throw new Error(
      'Tauri bridge unavailable. Launch the app through the desktop shell (pnpm dev:desktop) instead of a plain browser tab.',
    );
  }
  return tauri.core.invoke<T>(cmd);
}

function ReportValue({ value }: { value: unknown }) {
  if (value === null || value === undefined) {
    return <span className="muted">-</span>;
  }
  if (typeof value === 'boolean' || typeof value === 'number' || typeof value === 'string') {
    return <span>{String(value)}</span>;
  }
  if (Array.isArray(value)) {
    return (
      <ul className="report-list">
        {value.map((item, index) => (
          <li key={index}>
            <ReportValue value={item} />
          </li>
        ))}
      </ul>
    );
  }
  const entries = Object.entries(value as Record<string, unknown>);
  return (
    <dl className="report-grid">
      {entries.map(([key, child]) => (
        <div key={key} className="report-row">
          <dt>{key}</dt>
          <dd>
            <ReportValue value={child} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

interface PillState {
  label: string;
  className: string;
}

function derivePill(
  status: HostStatus | null,
  loadError: string | null,
  transition: Transition,
): PillState {
  if (transition === 'starting') {
    return { label: 'Starting...', className: 'pill pill-starting' };
  }
  if (transition === 'stopping') {
    return { label: 'Stopping...', className: 'pill pill-stopping' };
  }
  if (loadError && !status) {
    return { label: 'Error', className: 'pill pill-error' };
  }
  if (!status) {
    return { label: 'Connecting...', className: 'pill pill-connecting' };
  }
  if (status.connected) {
    if (status.servingStatus === null || status.servingStatus === 'SERVING') {
      return { label: 'Running', className: 'pill pill-running' };
    }
    return { label: `Degraded (${status.servingStatus})`, className: 'pill pill-degraded' };
  }
  if (status.error) {
    return { label: 'Stopped', className: 'pill pill-stopped' };
  }
  return { label: 'Stopped', className: 'pill pill-stopped' };
}

export function App() {
  const [status, setStatus] = useState<HostStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [transition, setTransition] = useState<Transition>('idle');
  const [actionError, setActionError] = useState<string | null>(null);
  const [doctorOpen, setDoctorOpen] = useState(false);
  const [doctorLoading, setDoctorLoading] = useState(false);
  const [doctor, setDoctor] = useState<DoctorResult | null>(null);
  const [reconnectedAt, setReconnectedAt] = useState<string | null>(null);
  const connectionHistoryRef = useRef<ConnectionHistory>('never-up');

  const poll = useCallback(async () => {
    try {
      const next = await invokeCommand<HostStatus>('host_status');
      setStatus(next);
      setLoadError(null);
      const previous = connectionHistoryRef.current;
      if (previous === 'down' && next.connected) {
        setReconnectedAt(new Date().toLocaleTimeString());
      }
      if (next.connected) {
        connectionHistoryRef.current = 'up';
      } else if (previous === 'up') {
        connectionHistoryRef.current = 'down';
      }
    } catch (err) {
      setStatus(null);
      setLoadError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      if (!cancelled && !document.hidden) {
        void poll();
      }
    };
    tick();
    const intervalId = window.setInterval(tick, POLL_INTERVAL_MS);
    document.addEventListener('visibilitychange', tick);
    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
      document.removeEventListener('visibilitychange', tick);
    };
  }, [poll]);

  const runLifecycleAction = useCallback(
    async (command: 'host_start' | 'host_stop', busy: Exclude<Transition, 'idle'>) => {
      if (transition !== 'idle') {
        return;
      }
      setTransition(busy);
      setActionError(null);
      try {
        const result = await invokeCommand<ActionResult>(command);
        if (!result.ok) {
          setActionError(result.error ?? result.detail ?? `${command} failed.`);
        }
      } catch (err) {
        setActionError(errorMessage(err));
      } finally {
        setTransition('idle');
        void poll();
      }
    },
    [transition, poll],
  );

  const runDoctor = useCallback(async () => {
    setDoctorOpen(true);
    setDoctorLoading(true);
    try {
      setDoctor(await invokeCommand<DoctorResult>('host_doctor'));
    } catch (err) {
      setDoctor({ ok: false, error: errorMessage(err) });
    } finally {
      setDoctorLoading(false);
    }
  }, []);

  const busy = transition !== 'idle';
  const pill = derivePill(status, loadError, transition);

  return (
    <main className="shell">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>

      {reconnectedAt ? (
        <div className="reconnect-banner" role="status">
          <span>Host reconnected after restart at {reconnectedAt}</span>
          <button type="button" className="link-button" onClick={() => setReconnectedAt(null)}>
            Dismiss
          </button>
        </div>
      ) : null}

      <section className="host-status" aria-live="polite" aria-busy={loading}>
        <div className="status-header">
          <span className={pill.className}>{pill.label}</span>
        </div>

        {actionError ? (
          <p role="alert" className="error">
            {actionError}
          </p>
        ) : null}

        {loading && !status ? (
          <p className="muted">Connecting to host at http://127.0.0.1:50051...</p>
        ) : loadError && !status ? (
          <>
            <p role="alert" className="error">
              {loadError}
            </p>
            <button type="button" onClick={() => void poll()}>
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
          </>
        ) : status ? (
          <p role="alert" className="error">
            {status.error ?? 'Host is not running.'}
          </p>
        ) : null}

        <div className="actions">
          <button
            type="button"
            disabled={busy || (status?.connected ?? false)}
            onClick={() => void runLifecycleAction('host_start', 'starting')}
          >
            Start Host
          </button>
          <button
            type="button"
            disabled={busy || !status?.connected}
            onClick={() => void runLifecycleAction('host_stop', 'stopping')}
          >
            Stop Host
          </button>
          <button type="button" disabled={busy} onClick={() => void runDoctor()}>
            Run Doctor
          </button>
          <button type="button" disabled={busy} onClick={() => void poll()}>
            Refresh
          </button>
        </div>
      </section>

      {doctorOpen ? (
        <section className="doctor-panel" aria-label="Doctor report">
          <div className="doctor-header">
            <h2>Doctor</h2>
            <button type="button" className="link-button" onClick={() => setDoctorOpen(false)}>
              Close
            </button>
          </div>
          {doctorLoading ? (
            <p className="muted">Running diagnostics...</p>
          ) : doctor ? (
            doctor.ok ? (
              <ReportValue value={doctor.report ?? {}} />
            ) : (
              <p role="alert" className="error">
                {doctor.error ?? 'Doctor reported failures.'}
                {doctor.report !== undefined ? <ReportValue value={doctor.report} /> : null}
              </p>
            )
          ) : null}
        </section>
      ) : null}
    </main>
  );
}
