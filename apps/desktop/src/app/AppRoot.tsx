import { QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { useEffect, type ReactNode } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { RouteErrorFallback } from '../components/RouteErrorFallback';
import { ToastViewport } from '../components/Toasts';
import { connectionManager } from '../lib/host/production-connection';
import { ThemeProvider } from '../theme/ThemeProvider';
import { createAppQueryClient } from './query-client';
import { router } from './router';

const queryClient = createAppQueryClient();

export function AppRoot(): ReactNode {
  // The connection manager singleton owns the app-lifetime Host connection;
  // starting it here means every later surface reads live state for free.
  useEffect(() => {
    void connectionManager.start();
  }, []);

  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <ErrorBoundary
          fallback={(props) => <RouteErrorFallback error={props.error} reset={props.reset} />}
        >
          <RouterProvider router={router} />
        </ErrorBoundary>
        <ToastViewport />
      </QueryClientProvider>
    </ThemeProvider>
  );
}
