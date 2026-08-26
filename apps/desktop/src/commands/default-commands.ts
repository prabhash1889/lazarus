import { useRouter } from '@tanstack/react-router';
import { useEffect } from 'react';

import { pushToast } from '../state/toast-store';
import { usePaletteStore } from '../state/palette-store';
import { connectionManager } from '../lib/host/production-connection';
import { useConnectionStore } from '../lib/host/connection-store';
import { createStubEpic, useShellStore } from '../state/shell-store';
import { useTheme } from '../theme/ThemeProvider';
import { useCommandRegistry } from './command-registry';

/**
 * Registers the Phase 3.3 baseline command set. Returns nothing; effects
 * manage registration lifecycle automatically.
 */
export function useRegisterAppCommands(): void {
  const router = useRouter();
  const { toggle, resolved } = useTheme();

  useEffect(() => {
    const { register } = useCommandRegistry.getState();
    const offHandlers = [
      register({
        id: 'shell.commandPalette',
        title: 'Open command palette',
        section: 'Shell',
        keywords: ['commands', 'search', 'actions'],
        shortcut: 'mod+k',
        run: () => usePaletteStore.getState().togglePalette(),
      }),
      register({
        id: 'shell.toggleAppearance',
        title: `Switch to ${resolved === 'dark' ? 'light' : 'dark'} appearance`,
        section: 'Shell',
        keywords: ['theme', 'dark', 'light', 'appearance'],
        shortcut: 'mod+j',
        run: toggle,
      }),
      register({
        id: 'nav.home',
        title: 'Go to Home',
        section: 'Navigate',
        keywords: ['home', 'start', 'dashboard'],
        shortcut: 'g h',
        run: () => void router.navigate({ to: '/' }),
      }),
      register({
        id: 'nav.draft',
        title: 'Go to Draft',
        section: 'Navigate',
        keywords: ['draft', 'compose', 'new'],
        shortcut: 'g d',
        run: () => void router.navigate({ to: '/draft' }),
      }),
      register({
        id: 'nav.history',
        title: 'Go to History',
        section: 'Navigate',
        keywords: ['history', 'tasks', 'previous', 'recent'],
        run: () => void router.navigate({ to: '/history' }),
      }),
      register({
        id: 'nav.settings',
        title: 'Go to Settings',
        section: 'Navigate',
        keywords: ['settings', 'preferences', 'options'],
        shortcut: 'g s',
        run: () => void router.navigate({ to: '/settings' }),
      }),
      register({
        id: 'nav.hostStatus',
        title: 'Go to Host status',
        section: 'Navigate',
        keywords: ['host', 'status', 'health', 'monitor'],
        shortcut: 'g m',
        run: () => void router.navigate({ to: '/host-status' }),
      }),
      register({
        id: 'nav.settings.providers',
        title: 'Settings: Providers',
        section: 'Navigate',
        keywords: ['providers', 'models', 'adapters'],
        run: () => void router.navigate({ to: '/settings/providers' }),
      }),
      register({
        id: 'nav.settings.appearance',
        title: 'Settings: Appearance',
        section: 'Navigate',
        keywords: ['appearance', 'theme', 'light', 'dark'],
        run: () => void router.navigate({ to: '/settings/appearance' }),
      }),
      register({
        id: 'nav.settings.usage',
        title: 'Settings: Usage and budget',
        section: 'Navigate',
        keywords: ['usage', 'budget', 'cost', 'tokens'],
        run: () => void router.navigate({ to: '/settings/usage' }),
      }),
      register({
        id: 'nav.settings.keybindings',
        title: 'Settings: Keybindings',
        section: 'Navigate',
        keywords: ['keybindings', 'shortcuts', 'keyboard', 'cheatsheet'],
        shortcut: 'mod+/',
        run: () => void router.navigate({ to: '/settings/keybindings' }),
      }),
      register({
        id: 'nav.settings.notifications',
        title: 'Settings: Notifications',
        section: 'Navigate',
        keywords: ['notifications', 'toasts', 'alerts'],
        run: () => void router.navigate({ to: '/settings/notifications' }),
      }),
      register({
        id: 'nav.settings.diagnostics',
        title: 'Settings: Diagnostics',
        section: 'Navigate',
        keywords: ['diagnostics', 'doctor', 'logs', 'troubleshooting'],
        run: () => void router.navigate({ to: '/settings/diagnostics' }),
      }),
      register({
        id: 'host.retryConnection',
        title: 'Retry Host connection',
        section: 'Host',
        keywords: ['retry', 'reconnect', 'connection'],
        when: () => {
          const phase = useConnectionStore.getState().phase;
          return phase === 'reconnecting' || phase === 'auth-failed';
        },
        run: () => void connectionManager.retryNow(),
      }),
      register({
        id: 'workspace.open',
        title: 'Open workspace...',
        section: 'Workspace',
        keywords: ['open', 'folder', 'repository', 'workspace'],
        run: () =>
          pushToast({
            kind: 'info',
            title: 'Workspaces arrive in Phase 4',
            detail: 'Repository registration lands with the workspace subsystem.',
          }),
      }),
      register({
        id: 'task.new',
        title: 'New Epic tab',
        section: 'Tasks',
        keywords: ['new', 'epic', 'task', 'create'],
        run: () => {
          const entity = createStubEpic();
          void router.navigate({ to: `/epic/${entity.id}` });
          pushToast({
            kind: 'info',
            title: 'Stub Epic opened',
            detail: 'Durable Tasks with persistence arrive in Phase 8.',
          });
        },
      }),
      register({
        id: 'tab.close',
        title: 'Close current Epic tab',
        section: 'Shell',
        keywords: ['close', 'tab', 'epic'],
        when: () => {
          const { activeTab } = useShellStore.getState();
          return useShellStore.getState().epicTabs.includes(activeTab);
        },
        run: () => {
          const { activeTab, closeEpicTab } = useShellStore.getState();
          closeEpicTab(activeTab);
          const nextActive = useShellStore.getState().activeTab;
          const path =
            nextActive === 'draft'
              ? '/draft'
              : nextActive === 'history'
                ? '/history'
                : nextActive === 'settings'
                  ? '/settings'
                  : `/epic/${nextActive}`;
          void router.navigate({ to: path });
        },
      }),
    ];
    return () => {
      for (const off of offHandlers) {
        off();
      }
    };
  }, [router, toggle, resolved]);
}
