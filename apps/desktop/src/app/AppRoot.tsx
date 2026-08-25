import { QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { type ReactNode } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { RouteErrorFallback } from '../components/RouteErrorFallback';
import { ToastViewport } from '../components/Toasts';
import { ThemeProvider } from '../theme/ThemeProvider';
import { createAppQueryClient } from './query-client';
import { router } from './router';

const queryClient = createAppQueryClient();

export function AppRoot(): ReactNode {
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
