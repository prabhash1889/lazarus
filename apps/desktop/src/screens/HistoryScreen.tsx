import { type ReactNode } from 'react';

/** Phase 3.4 placeholder; the filterable Task history lands with the screens. */
export default function HistoryScreen(): ReactNode {
  return (
    <main className="shell" data-testid="history-placeholder">
      <h1>History</h1>
      <p className="muted">Task history loading…</p>
    </main>
  );
}
