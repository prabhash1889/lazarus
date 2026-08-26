import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';

import HistoryScreen from './HistoryScreen';

describe('history screen', () => {
  afterEach(() => {
    cleanup();
  });

  it('renders stub history rows with statuses', async () => {
    render(<HistoryScreen />);
    expect(await screen.findByTestId('history-screen')).toBeTruthy();
    expect(
      screen.getAllByText(/COMPLETED|RUNNING|PENDING|FAILED|CANCELLED/).length,
    ).toBeGreaterThan(0);
  });

  it('filters by status', async () => {
    const user = userEvent.setup();
    const { container } = render(<HistoryScreen />);

    await user.click(screen.getByTestId('history-status-COMPLETED'));

    const pills = Array.from(container.querySelectorAll('.history-row .pill')).map(
      (el) => el.textContent,
    );
    expect(pills.length).toBeGreaterThan(0);
    expect(pills.every((status) => status === 'COMPLETED')).toBe(true);
  });

  it('filters by title text and shows the empty state when nothing matches', async () => {
    const user = userEvent.setup();
    render(<HistoryScreen />);

    await user.type(screen.getByTestId('history-query'), 'OAuth');
    expect(screen.getAllByText(/OAuth/).length).toBeGreaterThan(0);
    expect(screen.queryAllByText(/Terminal replay/)).toHaveLength(0);

    await user.clear(screen.getByTestId('history-query'));
    await user.type(screen.getByTestId('history-query'), 'zzz-no-such-task');
    expect(screen.getByTestId('history-empty')).toBeTruthy();
  });
});
