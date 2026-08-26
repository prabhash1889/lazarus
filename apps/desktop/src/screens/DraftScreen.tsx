import { type ReactNode } from 'react';

/** Phase 3.4 placeholder; the composer surface lands with the screens. */
export default function DraftScreen(): ReactNode {
  return (
    <main className="shell" data-testid="draft-placeholder">
      <h1>Draft</h1>
      <p className="muted">Composer loading…</p>
    </main>
  );
}
