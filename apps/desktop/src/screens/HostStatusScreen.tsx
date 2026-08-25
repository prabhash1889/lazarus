import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { describeError } from '../components/RouteErrorFallback';
import { invokeCommand } from '../lib/tauri';

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

function derivePill(status: HostStatus | undefined, isPending: boolean): { label: string; className: string } {
  if (!status) {
    return isPending
      ? { label: 'Connecting...', className: 'pill pill-connecting' }
      : { label: 'Error', className: 'pill pill-error' };
  }
  if (status.connected) {
    if (status.servingStatus === null || status.servingStatus === 'SERVING') {
      return { label: 'Running', className: 'pill pill-running' };
    }
    return { label: `Degraded (${status.servingStatus})`, className: 'pill pill-degraded' };
  }
  return { label: 'Stopped', className: 'pill pill-stopped' };
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
  const queryClient = useQueryClient();
  const [actionError, setActionError] = useState<string | null>(null);
  const [doctorOpen, setDoctorOpen] = useState(false);

  const statusQuery = useQuery({
    queryKey: ['host', 'status'],
    queryFn: () => invokeCommand<HostStatus>('host_status'),
    refetchInterval: POLL_INTERVAL_MS,
  });

  const lifecycleMutation = useMutation({
    mutationFn: (command: 'host_start' | 'host_stop') => invokeCommand<ActionResult>(command),
    onSuccess: (result) => {
      if (!result.ok) {
        setActionError(result.error ?? result.detail ?? 'Command failed.');
      }
    },
    onError: (error) => setActionError(describeError(error)),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: ['host'] }),
  });

  const doctorMutation = useMutation({
    mutationFn: () => invokeCommand<DoctorResult>('host_doctor'),
  });

  const status = statusQuery.data;
  const busy = lifecycleMutation.isPending;
  const pill = derivePill(status, statusQuery.isPending);
  const doctor = doctorMutation.data;

  return (
    <main className="shell">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>

      <section className="host-status" aria-live="polite" aria-busy={statusQuery.isFetching}>
        <div className="status-header">
          <span className={pill.className}>{pill.label}</span>
        </div>

        {actionError ? (
          <p role="alert" className="error">
            {actionError}
          </p>
        ) : null}

        {statusQuery.isError ? (
          <>
            <p role="alert" className="error">
              {describeError(statusQuery.error)}
            </p>
            <button type="button" onClick={() => void statusQuery.refetch()}>
              Retry
            </button>
          </>
        ) : null}

        {status && status.connected ? (
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
            onClick={() => lifecycleMutation.mutate('host_start')}
          >
            Start Host
          </button>
          <button
            type="button"
            disabled={busy || !status?.connected}
            onClick={() => lifecycleMutation.mutate('host_stop')}
          >
            Stop Host
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setDoctorOpen(true);
              doctorMutation.mutate();
            }}
          >
            Run Doctor
          </button>
          <button type="button" disabled={busy} onClick={() => void statusQuery.refetch()}>
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
        </section>
      ) : null}
    </main>
  );
}
