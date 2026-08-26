import { Link, Outlet } from '@tanstack/react-router';
import { type ReactNode } from 'react';

interface SettingsSection {
  to: string;
  label: string;
  description: string;
}

export const SETTINGS_SECTIONS: SettingsSection[] = [
  { to: '/settings/providers', label: 'Providers', description: 'Provider packs and models' },
  { to: '/settings/appearance', label: 'Appearance', description: 'Light/dark theme' },
  { to: '/settings/usage', label: 'Usage & budget', description: 'Tokens, cost, alerts' },
  { to: '/settings/keybindings', label: 'Keybindings', description: 'Shortcut cheat sheet' },
  { to: '/settings/notifications', label: 'Notifications', description: 'Toast preferences' },
  { to: '/settings/diagnostics', label: 'Diagnostics', description: 'Host health and logs' },
];

export default function SettingsScreen(): ReactNode {
  return (
    <div className="settings-layout">
      <nav className="settings-nav" aria-label="Settings sections">
        <ul>
          {SETTINGS_SECTIONS.map((section) => (
            <li key={section.to}>
              <Link
                to={section.to}
                className="settings-nav-link"
                activeProps={{ 'data-status': 'active' }}
                activeOptions={{ exact: true }}
              >
                <span className="settings-nav-label">{section.label}</span>
                <span className="settings-nav-description">{section.description}</span>
              </Link>
            </li>
          ))}
        </ul>
      </nav>
      <div className="settings-panel-host">
        <Outlet />
      </div>
    </div>
  );
}
