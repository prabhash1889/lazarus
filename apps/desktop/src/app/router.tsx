import {
  createHashHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Link,
  Outlet,
  useRouter,
  useRouterState,
  type RouterHistory,
} from '@tanstack/react-router';
import { lazy, useEffect, type ReactNode } from 'react';
import { CommandHost } from '../commands/CommandHost';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { RouteErrorFallback } from '../components/RouteErrorFallback';
import { RoutePending } from '../components/RoutePending';
import { TabStrip, tabPath } from '../components/tabstrip/TabStrip';
import { ThemeToggle } from '../components/ThemeToggle';
import { DEEP_LINK_EVENT, isNavTarget, NAVIGATE_EVENT } from '../lib/shell-events';
import { listenToEvent } from '../lib/tauri';
import {
  AppearancePanel,
  DiagnosticsPanel,
  KeybindingsPanel,
  NotificationsPanel,
  ProvidersPanel,
  UsageBudgetPanel,
} from '../screens/settings/panels';
import { isPinnedTab, useShellStore, type TabId } from '../state/shell-store';
import { useEpicsStore } from '../state/epics-store';

const HomeLazy = lazy(() => import('../screens/HomeScreen'));
const HostStatusLazy = lazy(() => import('../screens/HostStatusScreen'));
const SettingsLazy = lazy(() => import('../screens/settings/SettingsScreen'));
const DraftLazy = lazy(() => import('../screens/DraftScreen'));
const HistoryLazy = lazy(() => import('../screens/HistoryScreen'));
const EpicLazy = lazy(() => import('../screens/EpicScreen'));
const EngineMatrixLazy = lazy(() => import('../screens/EngineMatrixScreen'));

/** Maps the current location onto its shell tab id. */
export function tabIdForPath(pathname: string): TabId | null {
  if (pathname === '/' || pathname === '') {
    return null; // Home is a transient screen, not a tab.
  }
  for (const pinned of ['draft', 'history'] as const) {
    if (pathname === `/${pinned}`) {
      return pinned;
    }
  }
  if (pathname.startsWith('/settings')) {
    return 'settings';
  }
  if (pathname.startsWith('/epic/')) {
    return decodeURIComponent(pathname.slice('/epic/'.length));
  }
  return null;
}

function RootLayout(): ReactNode {
  const router = useRouter();
  const pathname = useRouterState({ select: (state) => state.location.pathname });

  // The strip mirrors the router: navigating anywhere opens or activates
  // the matching tab, so deep links and palette commands behave identically.
  const tabId = tabIdForPath(pathname);
  useEffect(() => {
    if (tabId === null) {
      return;
    }
    const shell = useShellStore.getState();
    if (!isPinnedTab(tabId) && !shell.epicTabs.includes(tabId)) {
      shell.openEpic(tabId);
    } else if (shell.activeTab !== tabId) {
      shell.setActiveTab(tabId);
    }
  }, [tabId]);

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

  const onSelectTab = (id: TabId): void => {
    void router.navigate({ to: tabPath(id) });
  };
  const onNewEpic = (): void => {
    // Stub entity creation until durable Tasks arrive in Phase 8.
    const entity = useEpicsStore.getState().createEpic();
    useShellStore.getState().openEpic(entity.id);
    void router.navigate({ to: `/epic/${entity.id}` });
  };

  return (
    <ErrorBoundary
      fallback={(props) => <RouteErrorFallback error={props.error} reset={props.reset} />}
    >
      <div className="app-frame">
        <header className="app-header app-header-with-tabs">
          <Link to="/" className="app-brand">
            Lazarus
          </Link>
          <nav className="app-nav" aria-label="Transient screens">
            <Link to="/" className="nav-link nav-link-small">
              Home
            </Link>
            <Link to="/host-status" className="nav-link nav-link-small">
              Host Status
            </Link>
          </nav>
          <TabStrip
            activeTab={tabId ?? 'draft'}
            onSelect={onSelectTab}
            onClose={(epicId) => useShellStore.getState().closeEpicTab(epicId)}
            onReorder={(from, to) => useShellStore.getState().moveTab(from, to)}
            onNewEpic={onNewEpic}
          />
          <ThemeToggle />
        </header>
        <div className="app-content">
          <Outlet />
        </div>
      </div>
      <CommandHost />
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

const draftRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/draft',
  component: DraftLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const historyRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/history',
  component: HistoryLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const epicRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/epic/$taskId',
  component: EpicLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const engineMatrixRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/engine-matrix',
  component: EngineMatrixLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/settings',
  component: SettingsLazy,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

function settingsChild(
  path: 'providers' | 'appearance' | 'usage' | 'keybindings' | 'notifications' | 'diagnostics',
  component: () => ReactNode,
) {
  return createRoute({
    getParentRoute: () => settingsRoute,
    path,
    component,
    pendingComponent: RoutePending,
    errorComponent: RouteErrorFallback,
  });
}

const settingsIndexRoute = createRoute({
  getParentRoute: () => settingsRoute,
  path: '/',
  component: ProvidersPanel,
  pendingComponent: RoutePending,
  errorComponent: RouteErrorFallback,
});

const settingsRouteTree = settingsRoute.addChildren([
  settingsIndexRoute,
  settingsChild('providers', ProvidersPanel),
  settingsChild('appearance', AppearancePanel),
  settingsChild('usage', UsageBudgetPanel),
  settingsChild('keybindings', KeybindingsPanel),
  settingsChild('notifications', NotificationsPanel),
  settingsChild('diagnostics', DiagnosticsPanel),
]);

const routeTree = rootRoute.addChildren([
  indexRoute,
  hostStatusRoute,
  draftRoute,
  historyRoute,
  epicRoute,
  engineMatrixRoute,
  settingsRouteTree,
]);

export function createAppRouter(history?: RouterHistory) {
  return createRouter({
    routeTree,
    history: history ?? createHashHistory(),
    defaultPendingComponent: RoutePending,
    defaultErrorComponent: RouteErrorFallback,
    defaultPreload: 'intent',
  });
}

export const router = createAppRouter();

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}
