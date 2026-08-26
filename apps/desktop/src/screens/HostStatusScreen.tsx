import { useMutation } from '@tanstack/react-query';
import { useEffect, useState, type ReactNode } from 'react';

import { Button } from '../components/Button';
import { Dialog } from '../components/Dialog';
import { describeError } from '../components/RouteErrorFallback';
import { connectionManager } from '../lib/host/production-connection';
import {
  selectMethodLabel,
  supportLabel,
  useConnectionStore,
  type ConnectionPhase,
} from '../lib/host/connection-store';
import { formatUptimeSeconds, uptimeSecondsFrom } from '../lib/host/uptime';
import { invokeCommand } from '../lib/tauri';

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

const PHASE_PILLS: Record<ConnectionPhase, { label: string; className: string }> = {
  disconnected: { label: 'Disconnected', className: 'pill pill-stopped' },
  connecting: { label: 'Connecting...', className: 'pill pill-connecting' },
  authenticated: { label: 'Connected', className: 'pill pill-running' },
  reconnecting: { label: 'Reconnecting...', className: 'pill pill-connecting' },
  'auth-failed': { label: 'Auth failed', className: 'pill pill-error' },
  degraded: { label: 'Degraded (not serving)', className: 'pill pill-degraded' },
};

function PhasePill(): ReactNode {
  const phase = useConnectionStore((state) => state.phase);
  const pill = PHASE_PILLS[phase];
  return (
    <span className={pill.className} data-phase={phase}>
      {pill.label}
    </span>
  );
}

function UptimeCell(): ReactNode {
  const startedAtUnixMs = useConnectionStore((state) => state.startedAtUnixMs);
  const [nowUnixMs, setNowUnixMs] = useState(() => Date.now());

  useEffect(() => {
    if (startedAtUnixMs === null) {
      return;
    }
    const handle = window.setInterval(() => setNowUnixMs(Date.now()), 1000);
    return () => window.clearInterval(handle);
  }, [startedAtUnixMs]);

  const seconds = uptimeSecondsFrom(startedAtUnixMs, nowUnixMs);
  if (seconds === null) {
    // Hosts older than the v1.1 getInfo contract do not report a stamp.
    return <span className="muted">unknown</span>;
  }
  return <span>{formatUptimeSeconds(seconds)}</span>;
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

export default function HostStatusScreen() {
  const [actionError, setActionError] = useState<string | null>(null);
  const [doctorOpen, setDoctorOpen] = useState(false);

  const phase = useConnectionStore((state) => state.phase);
  const lastErrorCode = useConnectionStore((state) => state.lastErrorCode);
  const lastErrorMessage = useConnectionStore((state) => state.lastErrorMessage);
  const hostVersion = useConnectionStore((state) => state.hostVersion);
  const servingStatus = useConnectionStore((state) => state.servingStatus);
  const capabilities = useConnectionStore((state) => state.capabilities);
  const methods = useConnectionStore((state) => state.methods);
  const outageId = useConnectionStore((state) => state.outageId);
  const liveSequence = useConnectionStore((state) => state.liveSequence);
  const reconnectAttempt = useConnectionStore((state) => state.reconnectAttempt);

  const lifecycleMutation = useMutation({
    mutationFn: async (command: 'host_start' | 'host_stop') => {
      const result = await invokeCommand<ActionResult>(command);
      if (!result.ok) {
        throw new Error(result.error ?? result.detail ?? 'Command failed.');
      }
      return result;
    },
    onSuccess: async (_result, command) => {
      setActionError(null);
      // Nudge the manager so the surface recovers without waiting out the
      // backoff window after an explicit start.
      if (command === 'host_start') {
        await connectionManager.retryNow();
      }
    },
    onError: (error) => setActionError(describeError(error)),
  });

  const doctorMutation = useMutation({
    mutationFn: () => invokeCommand<DoctorResult>('host_doctor'),
  });

  const busy = lifecycleMutation.isPending;
  const doctor = doctorMutation.data;
  const connected = phase === 'authenticated' || phase === 'degraded';

  return (
    <main className="shell">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>

      <section className="host-status" aria-live="polite">
        <div className="status-header">
          <PhasePill />
          {phase === 'reconnecting' || phase === 'auth-failed' ? (
            <span className="muted">attempt {reconnectAttempt}</span>
          ) : null}
        </div>

        {lastErrorMessage !== null && !connected ? (
          <p role="alert" className="error" data-error-code={lastErrorCode ?? undefined}>
            {lastErrorCode !== null ? `[${lastErrorCode}] ` : ''}
            {lastErrorMessage}
          </p>
        ) : null}
        {actionError ? (
          <p role="alert" className="error">
            {actionError}
          </p>
        ) : null}

        <dl className="status-grid">
          <dt>Host version</dt>
          <dd>{hostVersion ?? '-'}</dd>
          <dt>Serving status</dt>
          <dd>{servingStatus ?? '-'}</dd>
          <dt>Uptime</dt>
          <dd>
            <UptimeCell />
          </dd>
          <dt>Event stream</dt>
          <dd>
            {outageId === null ? (
              <span className="muted">not subscribed</span>
            ) : (
              <span>
                live{liveSequence !== null ? ` (seq ${liveSequence})` : ''} - incarnation{' '}
                <code>{outageId}</code>
              </span>
            )}
          </dd>
        </dl>

        {methods.length > 0 ? (
          <>
            <h2>Negotiated methods</h2>
            <ul className="capability-list" aria-label="Negotiated protocol methods">
              {methods.map((method) => (
                <li key={method.name}>
                  <code>{method.name}</code> - {selectMethodLabel(method)} (
                  {supportLabel(method.support)})
                </li>
              ))}
            </ul>
          </>
        ) : null}

        {connected && capabilities.length > 0 ? (
          <>
            <h2>Capabilities</h2>
            <ul className="capability-list">
              {capabilities.map((capability) => (
                <li key={capability.name}>
                  <code>{capability.name}</code> - {capability.enabled ? 'enabled' : 'disabled'}
                </li>
              ))}
            </ul>
          </>
        ) : null}

        {!connected && (phase === 'reconnecting' || phase === 'auth-failed') ? (
          <div className="actions">
            <Button variant="primary" onClick={() => void connectionManager.retryNow()}>
              Retry now
            </Button>
          </div>
        ) : null}

        <div className="actions">
          <Button
            variant="primary"
            disabled={busy || connected}
            onClick={() => lifecycleMutation.mutate('host_start')}
          >
            Start Host
          </Button>
          <Button
            variant="danger"
            disabled={busy || !connected}
            onClick={() => lifecycleMutation.mutate('host_stop')}
          >
            Stop Host
          </Button>
          <Button
            disabled={busy}
            onClick={() => {
              setDoctorOpen(true);
              doctorMutation.mutate();
            }}
          >
            Run Doctor
          </Button>
        </div>
      </section>

      <Dialog open={doctorOpen} onOpenChange={setDoctorOpen} title="Doctor">
        {doctorMutation.isPending ? (
          <p className="muted">Running diagnostics...</p>
        ) : doctorMutation.isError ? (
          <p role="alert" className="error">
            {describeError(doctorMutation.error)}
          </p>
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
      </Dialog>
    </main>
  );
}
