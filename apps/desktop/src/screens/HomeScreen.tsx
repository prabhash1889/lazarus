import { Link, useNavigate } from '@tanstack/react-router';
import { type ReactNode } from 'react';

import { Button } from '../components/Button';
import { VirtualList } from '../components/VirtualList';
import { useConnectionStore, type ConnectionPhase } from '../lib/host/connection-store';
import { pushToast } from '../state/toast-store';
import { useTheme } from '../theme/ThemeProvider';

const PHASE_LABELS: Record<ConnectionPhase, string> = {
  disconnected: 'Disconnected',
  connecting: 'Connecting',
  authenticated: 'Connected',
  reconnecting: 'Reconnecting',
  'auth-failed': 'Auth failed',
  degraded: 'Degraded',
};

export default function HomeScreen(): ReactNode {
  const navigate = useNavigate();
  const { toggle } = useTheme();
  const phase = useConnectionStore((state) => state.phase);
  const hostVersion = useConnectionStore((state) => state.hostVersion);
  const workspaces = useConnectionStore((state) => state.workspaces);
  const tasks = useConnectionStore((state) => state.tasks);

  const quickActions: Array<{
    id: string;
    label: string;
    primary?: boolean;
    onSelect: () => void;
  }> = [
    {
      id: 'new-task',
      label: 'New Task...',
      primary: true,
      onSelect: () =>
        pushToast({
          kind: 'info',
          title: 'Task creation arrives in Phase 8',
          detail: 'Durable Tasks land with persistence and agents.',
        }),
    },
    {
      id: 'open-workspace',
      label: 'Open workspace...',
      onSelect: () =>
        pushToast({
          kind: 'info',
          title: 'Workspaces arrive in Phase 4',
          detail: 'Repository registration lands with the workspace subsystem.',
        }),
    },
    {
      id: 'host-status',
      label: 'Host status',
      onSelect: () => void navigate({ to: '/host-status' }),
    },
    {
      id: 'settings',
      label: 'Settings',
      onSelect: () => void navigate({ to: '/settings' }),
    },
  ];

  return (
    <main className="shell home-screen">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>

      <section className="home-card" aria-label="Quick actions">
        <h2>Quick actions</h2>
        <div className="actions">
          {quickActions.map((action) => (
            <Button
              key={action.id}
              variant={action.primary ? 'primary' : 'ghost'}
              onClick={action.onSelect}
            >
              {action.label}
            </Button>
          ))}
          <Button onClick={toggle}>Toggle appearance</Button>
        </div>
        <p className="muted">
          Press <kbd data-testid="palette-hint">Ctrl+K</kbd> anywhere for the command palette.
        </p>
      </section>

      <section className="home-card" aria-label="Registered workspaces">
        <h2>Workspaces</h2>
        {workspaces.length === 0 ? (
          <p className="muted" data-testid="workspaces-empty">
            No workspaces registered yet. Repository registration arrives in Phase 4.
          </p>
        ) : (
          <VirtualList
            className="home-list"
            viewportHeight={180}
            rows={workspaces.map((workspace) => ({
              kind: 'item' as const,
              key: workspace.id,
              item: workspace,
            }))}
            rowHeight={() => 36}
            ariaLabel="Registered workspaces"
            renderItem={(workspace) => (
              <span title={workspace.id}>{workspace.name || workspace.id}</span>
            )}
          />
        )}
      </section>

      <section className="home-card" aria-label="Recent Tasks">
        <h2>Recent Tasks</h2>
        {tasks.length === 0 ? (
          <p className="muted" data-testid="tasks-empty">
            No recent Tasks. Durable Tasks arrive in Phase 8.
          </p>
        ) : (
          <VirtualList
            className="home-list"
            viewportHeight={180}
            rows={tasks.map((task) => ({ kind: 'item' as const, key: task.id, item: task }))}
            rowHeight={() => 36}
            ariaLabel="Recent Tasks"
            renderItem={(task) => <span title={task.id}>{task.title}</span>}
          />
        )}
      </section>

      <section className="home-card" aria-label="Host connection summary">
        <h2>Host</h2>
        <p className="muted" data-testid="home-connection">
          Host: {PHASE_LABELS[phase] ?? phase}
          {hostVersion !== null ? ` (${hostVersion})` : ''} - <Link to="/host-status">details</Link>
        </p>
      </section>
    </main>
  );
}
