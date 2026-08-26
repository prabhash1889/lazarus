import { useMemo, useState, type ReactNode } from 'react';

import { Button, joinClassNames } from '../components/Button';
import { VirtualList } from '../components/VirtualList';

/**
 * Prior Task history (Phase 3.4). Stub rows until Phase 8 delivers durable
 * Tasks; what matters now is the filter surface and the shared virtualized
 * list primitive handling large histories.
 */

interface HistoryEntry {
  id: string;
  title: string;
  status: 'PENDING' | 'RUNNING' | 'COMPLETED' | 'FAILED' | 'CANCELLED';
  updatedAt: string;
}

const STATUSES = ['ALL', 'PENDING', 'RUNNING', 'COMPLETED', 'FAILED', 'CANCELLED'] as const;

/** Deterministic stub history so tests and screenshots are stable. */
function stubHistory(): HistoryEntry[] {
  const titles = [
    'Add OAuth login',
    'Fix flaky invite test',
    'Split billing module',
    'Migrate artifacts to revisions',
    'Speed up worktree pruning',
    'Draft ADR on protocol bridges',
    'Repair loop budget guard',
    'Terminal replay buffer limits',
  ];
  const statuses: HistoryEntry['status'][] = [
    'COMPLETED',
    'RUNNING',
    'PENDING',
    'COMPLETED',
    'FAILED',
    'CANCELLED',
    'COMPLETED',
    'RUNNING',
  ];
  return Array.from({ length: 48 }, (_, index) => ({
    id: `0198e550-c9be-7000-8000-${(index + 1).toString().padStart(12, '0')}`,
    title: `${titles[index % titles.length]!} #${index + 1}`,
    status: statuses[index % statuses.length]!,
    updatedAt: `2026-08-${((index % 28) + 1).toString().padStart(2, '0')}T09:${((index * 7) % 60)
      .toString()
      .padStart(2, '0')}:00Z`,
  }));
}

export default function HistoryScreen(): ReactNode {
  const history = useMemo(stubHistory, []);
  const [statusFilter, setStatusFilter] = useState<(typeof STATUSES)[number]>('ALL');
  const [query, setQuery] = useState('');

  const filtered = history.filter((entry) => {
    if (statusFilter !== 'ALL' && entry.status !== statusFilter) {
      return false;
    }
    if (query.trim() !== '' && !entry.title.toLowerCase().includes(query.trim().toLowerCase())) {
      return false;
    }
    return true;
  });

  return (
    <main className="shell history-screen" data-testid="history-screen">
      <h1>History</h1>
      <p>Prior Tasks with their latest state.</p>

      <section className="home-card" aria-label="History filters">
        <div className="history-filters">
          <input
            value={query}
            data-testid="history-query"
            placeholder="Filter by title…"
            aria-label="Filter history by title"
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="actions" role="group" aria-label="Status filter">
            {STATUSES.map((status) => (
              <Button
                key={status}
                variant={statusFilter === status ? 'primary' : 'ghost'}
                data-testid={`history-status-${status}`}
                onClick={() => setStatusFilter(status)}
              >
                {status === 'ALL' ? 'All' : status}
              </Button>
            ))}
          </div>
        </div>
      </section>

      <section className="home-card history-results" aria-label="Task history">
        {filtered.length === 0 ? (
          <p className="muted" data-testid="history-empty">
            No Tasks match these filters.
          </p>
        ) : (
          <VirtualList
            viewportHeight={420}
            className="history-list"
            ariaLabel="Task history"
            rows={filtered.map((entry) => ({ kind: 'item' as const, key: entry.id, item: entry }))}
            rowHeight={() => 44}
            renderItem={(entry) => (
              <div className={joinClassNames('history-row')}>
                <span className="history-title">{entry.title}</span>
                <span className={`pill pill-${entry.status.toLowerCase()}`}>{entry.status}</span>
                <time className="muted">{entry.updatedAt.slice(0, 10)}</time>
              </div>
            )}
          />
        )}
        <p className="muted">Stub data - durable Task history arrives in Phase 8.</p>
      </section>
    </main>
  );
}
