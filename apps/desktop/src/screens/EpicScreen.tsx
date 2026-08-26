import { type ReactNode } from 'react';

/**
 * Phase 3.4 placeholder for the Epic canvas surface. The full persisted
 * tile canvas lands in the same phase's screen commit; this keeps routing,
 * tabs, and layout restoration honest while the canvas wires up.
 */
export default function EpicScreen(): ReactNode {
  return (
    <main className="shell" data-testid="epic-placeholder">
      <h1>Epic</h1>
      <p className="muted">Canvas loading…</p>
    </main>
  );
}
