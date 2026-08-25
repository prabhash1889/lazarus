interface RouteErrorFallbackProps {
  error: unknown;
  reset: () => void;
}

export function describeError(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error ?? 'Unknown error');
}

export function RouteErrorFallback({ error, reset }: RouteErrorFallbackProps) {
  return (
    <section className="route-fallback" role="alert">
      <h2>Something went wrong</h2>
      <p className="route-fallback-message">{describeError(error)}</p>
      <div className="route-fallback-actions">
        <button type="button" onClick={reset}>
          Try again
        </button>
        <button type="button" onClick={() => window.location.reload()}>
          Reload app
        </button>
      </div>
    </section>
  );
}
