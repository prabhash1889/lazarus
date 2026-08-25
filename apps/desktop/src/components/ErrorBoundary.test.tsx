import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ErrorBoundary } from './ErrorBoundary';

let explode = true;

function Boom() {
  if (explode) {
    throw new Error('kaboom');
  }
  return <p>recovered</p>;
}

describe('ErrorBoundary', () => {
  afterEach(() => {
    cleanup();
    explode = true;
    vi.restoreAllMocks();
  });

  it('renders the fallback with a recovery action instead of blanking', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    render(
      <ErrorBoundary fallback={({ error, reset }) => (
        <div>
          <p>{error.message}</p>
          <button type="button" onClick={reset}>
            Try again
          </button>
        </div>
      )}>
        <Boom />
      </ErrorBoundary>,
    );
    expect(screen.getByText('kaboom')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  });

  it('recovers when the reset action re-renders a healthy tree', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    render(
      <ErrorBoundary fallback={({ reset }) => (
        <button type="button" onClick={() => { explode = false; reset(); }}>
          Try again
        </button>
      )}>
        <Boom />
      </ErrorBoundary>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(screen.getByText('recovered')).toBeTruthy();
  });
});
