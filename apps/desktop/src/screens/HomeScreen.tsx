import { Link } from '@tanstack/react-router';
import { type ReactNode } from 'react';

import { Button } from '../components/Button';
import { useConnectionStore } from '../lib/host/connection-store';

const PHASE_LABELS: Record<string, string> = {
  disconnected: 'Disconnected',
  connecting: 'Connecting',
  authenticated: 'Connected',
  reconnecting: 'Reconnecting',
  'auth-failed': 'Auth failed',
  degraded: 'Degraded',
};

export default function HomeScreen(): ReactNode {
  const phase = useConnectionStore((state) => state.phase);
  const hostVersion = useConnectionStore((state) => state.hostVersion);

  return (
    <main className="shell">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>
      <section className="home-card">
        <h2>Welcome</h2>
        <p className="muted">
          This Home surface is a placeholder proving the shell, routing, and theme layers. Task,
          workspace, and agent features arrive in later phases.
        </p>
        <p className="muted" data-testid="home-connection">
          Host: {PHASE_LABELS[phase] ?? phase}
          {hostVersion !== null ? ` (${hostVersion})` : ''}
        </p>
        <div className="actions">
          <Button variant="primary" asChild>
            <Link to="/host-status">Open Host status</Link>
          </Button>
        </div>
      </section>
    </main>
  );
}
