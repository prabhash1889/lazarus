import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ThemeProvider, useTheme } from './ThemeProvider';

function stubMatchMedia(matches: boolean): void {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    configurable: true,
    value: (query: string) => ({
      matches,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }),
  });
}

function Probe() {
  const { resolved, toggle } = useTheme();
  return (
    <div>
      <span data-testid="resolved">{resolved}</span>
      <button type="button" onClick={toggle}>
        toggle
      </button>
    </div>
  );
}

describe('ThemeProvider', () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.documentElement.dataset.theme = '';
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('applies the resolved theme to the document element and persists the choice', () => {
    stubMatchMedia(false);
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(document.documentElement.dataset.theme).toBe('light');
    fireEvent.click(screen.getByRole('button', { name: 'toggle' }));
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(window.localStorage.getItem('lazarus.theme')).toBe('dark');
  });

  it('restores a stored choice instead of the system preference', () => {
    stubMatchMedia(false);
    window.localStorage.setItem('lazarus.theme', 'dark');
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(screen.getByTestId('resolved').textContent).toBe('dark');
  });

  it('follows the system preference while the choice is system', () => {
    stubMatchMedia(true);
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(window.localStorage.getItem('lazarus.theme')).toBe('system');
  });
});
