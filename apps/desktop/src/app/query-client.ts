import { QueryCache, QueryClient } from '@tanstack/react-query';
import { pushToast } from '../state/toast-store';

export function createAppQueryClient(): QueryClient {
  return new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        pushToast({
          kind: 'error',
          title: 'Request failed',
          detail: `${query.queryKey.join('.')}: ${error instanceof Error ? error.message : String(error)}`,
        });
      },
    }),
    defaultOptions: {
      queries: {
        retry: 2,
        retryDelay: (attempt) => Math.min(1000 * 2 ** attempt, 30_000),
        staleTime: 15_000,
        gcTime: 5 * 60_000,
        refetchOnWindowFocus: true,
      },
      mutations: {
        retry: 0,
      },
    },
  });
}
