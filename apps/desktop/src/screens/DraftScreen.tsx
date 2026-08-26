import { useNavigate } from '@tanstack/react-router';
import { useState, type FormEvent, type ReactNode } from 'react';

import { Button } from '../components/Button';
import { createStubEpic } from '../state/shell-store';
import { pushToast } from '../state/toast-store';

/**
 * The pre-Task composer (Phase 3.4 placeholder). Capturing a draft as a
 * stub Epic proves the shell loop end to end; durable Task creation with
 * specs, worktrees, and agents replaces this flow in Phase 8.
 */
export default function DraftScreen(): ReactNode {
  const navigate = useNavigate();
  const [title, setTitle] = useState('');
  const [notes, setNotes] = useState('');

  const openAsEpic = (event: FormEvent): void => {
    event.preventDefault();
    if (title.trim() === '' && notes.trim() === '') {
      pushToast({
        kind: 'info',
        title: 'Nothing to draft yet',
        detail: 'Give the draft a title or a few notes first.',
      });
      return;
    }
    const entity = createStubEpic(title.trim() || undefined);
    void navigate({ to: `/epic/${entity.id}` });
    setTitle('');
    setNotes('');
  };

  return (
    <main className="shell draft-screen" data-testid="draft-screen">
      <h1>Draft</h1>
      <p>Sketch an idea now; turn it into an Epic workspace when it is ready.</p>
      <form className="home-card draft-card" onSubmit={openAsEpic} aria-label="Draft composer">
        <label className="draft-field">
          Title
          <input
            value={title}
            data-testid="draft-title"
            placeholder="What is this about?"
            onChange={(event) => setTitle(event.target.value)}
          />
        </label>
        <label className="draft-field">
          Notes
          <textarea
            value={notes}
            rows={6}
            data-testid="draft-notes"
            placeholder="Goals, links, constraints - anything worth keeping."
            onChange={(event) => setNotes(event.target.value)}
          />
        </label>
        <div className="actions">
          <Button type="submit" variant="primary" data-testid="draft-open-epic">
            Open as Epic
          </Button>
        </div>
        <p className="muted">Draft-to-Task conversion with specs and agents arrives in Phase 8.</p>
      </form>
    </main>
  );
}
