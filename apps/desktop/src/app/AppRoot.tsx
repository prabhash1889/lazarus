import { QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { type ReactNode } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { RouteErrorFallback } from '../components/RouteErrorFallback';
import { createAppQueryClient } from './query-client';
import { router } from './router';

const queryClient = createAppQueryClient();

export function AppRoot(): ReactNode {
  return (
    <QueryClientProvider client={queryClient}>
      <ErrorBoundary
        fallback={(props) => <RouteErrorFallback error={props.error} reset={props.reset} />}
      >
        <RouterProvider router={router} />
      </ErrorBoundary>
    </QueryClientProvider>
  );
}
