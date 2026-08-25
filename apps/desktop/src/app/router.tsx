import {
  createHashHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Link,
  Outlet,
  useRouter,
} from '@tanstack/react-router';
import { lazy, useEffect, type ReactNode } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { RouteErrorFallback } from '../components/RouteErrorFallback';
import { RoutePending } from '../components/RoutePending';
import { ThemeToggle } from '../components/ThemeToggle';
import { DEEP_LINK_EVENT, isNavTarget, NAVIGATE_EVENT } from '../lib/shell-events';
import { listenToEvent } from '../lib/tauri';

const HomeLazy = lazy(() => import('../screens/HomeScreen'));
const HostStatusLazy = lazy(() => import('../screens/HostStatusScreen'));

function RootLayout(): ReactNode {
  const router = useRouter();

  useEffect(() => {
    const unlistenNav = listenToEvent<string>(NAVIGATE_EVENT, (path) => {
      if (isNavTarget(path)) {
        void router.navigate({ to: path });
      }
    });
    const unlistenDeepLink = listenToEvent<string[]>(DEEP_LINK_EVENT, (urls) => {
      console.info('Deep link received; validation lands in Phase 20.', urls);
    });
    return () => {
      void unlistenNav.then((off) => off());
      void unlistenDeepLink.then((off) => off());
    };
  }, [router]);

  return (
    <ErrorBoundary
      fallback={(props) => <RouteErrorFallback error={props.error} reset={props.reset} />}
    >
      <div className="app-frame">
        <header className="app-header">
          <Link to="/" className="app-brand">
            Lazarus
          </Link>
          <nav className="app-nav" aria-label="Primary">
            <Link to="/" className="nav-link">
              Home
            </Link>
            <Link to="/host-status" className="nav-link">
              Host Status
            </Link>
          </nav>
          <ThemeToggle />
        </header>
        <div className="app-content">
          <Outlet />
        </div>
      </div>
    </ErrorBoundary>
  );
}

const rootRoute = createRootRoute({
  component: RootLayout,
  errorComponent: RouteErrorFallback,
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: HomeLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const hostStatusRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/host-status',
  component: HostStatusLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const routeTree = rootRoute.addChildren([indexRoute, hostStatusRoute]);

export const router = createRouter({
  routeTree,
  history: createHashHistory(),
  defaultPendingComponent: RoutePending,
  defaultErrorComponent: RouteErrorFallback,
  defaultPreload: 'intent',
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}
