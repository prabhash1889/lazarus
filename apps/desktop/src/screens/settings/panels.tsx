import { type ReactNode } from 'react';

import { useCommandRegistry } from '../../commands/command-registry';
import { formatShortcut, parseShortcut } from '../../commands/shortcut-keys';
import { Link } from '@tanstack/react-router';
import { useTheme, type ThemeChoice } from '../../theme/ThemeProvider';

function Panel(props: { title: string; phase: string; children?: ReactNode }): ReactNode {
  return (
    <section className="settings-panel" data-testid="settings-panel">
      <h2>{props.title}</h2>
      <p className="muted">{props.phase}</p>
      {props.children}
    </section>
  );
}

export function ProvidersPanel(): ReactNode {
  return (
    <Panel title="Providers" phase="Placeholder - provider packs land in Phase 12.">
      <p className="muted">
        Installed coding-agent CLIs, API providers, model lists, capability probes, and default
        models will be managed here. Provider-specific behavior stays inside provider adapters; the
        core never branches on a provider name.
      </p>
    </Panel>
  );
}

export function AppearancePanel(): ReactNode {
  const { choice, resolved, setTheme } = useTheme();
  const options: Array<{ value: ThemeChoice; label: string }> = [
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
    { value: 'system', label: `System (currently ${resolved})` },
  ];
  return (
    <Panel title="Appearance" phase="Active - theme tokens are live since Phase 3.1.">
      <div role="radiogroup" aria-label="Appearance">
        {options.map((option) => (
          <label key={option.value} className="appearance-option">
            <input
              type="radio"
              name="theme"
              value={option.value}
              checked={choice === option.value}
              onChange={() => setTheme(option.value)}
            />
            <span>{option.label}</span>
          </label>
        ))}
      </div>
    </Panel>
  );
}

export function UsageBudgetPanel(): ReactNode {
  return (
    <Panel title="Usage & budget" phase="Placeholder - the usage ledger lands with agents.">
      <p className="muted">
        Per-agent and per-task token/cost accounting, budget thresholds, and rate-limit status will
        be surfaced here once provider runs report usage.
      </p>
    </Panel>
  );
}

export function NotificationsPanel(): ReactNode {
  return (
    <Panel title="Notifications" phase="Placeholder - notification preferences land later.">
      <p className="muted">
        Connection events already feed the toast subsystem. Per-surface notification controls and
        quiet hours will be configured here.
      </p>
    </Panel>
  );
}

export function DiagnosticsPanel(): ReactNode {
  return (
    <Panel
      title="Diagnostics"
      phase="Placeholder - deep diagnostics arrive with Phase 2.5+ surfaces."
    >
      <p className="muted">
        Structured logs, metrics, and <code>lazarus doctor</code> reports will be viewable here.
      </p>
      <div className="actions">
        <Link to="/host-status" className="btn btn-primary">
          Open Host status
        </Link>
      </div>
    </Panel>
  );
}

export function KeybindingsPanel(): ReactNode {
  const commands = useCommandRegistry((state) => state.commands);
  const entries = Object.values(commands).filter((command) => command.shortcut);
  entries.sort((a, b) => a.title.localeCompare(b.title));
  const rejected = Object.values(commands).filter(
    (command) => command.shortcutRejectedBy !== undefined,
  );

  return (
    <Panel title="Keybindings" phase="Cheat sheet - every registered shortcut and its command.">
      {entries.length === 0 ? (
        <p className="muted">No shortcuts registered.</p>
      ) : (
        <table className="keybindings-table">
          <thead>
            <tr>
              <th scope="col">Shortcut</th>
              <th scope="col">Command</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((command) => (
              <tr key={command.id}>
                <td>
                  <kbd>{formatBinding(command)}</kbd>
                </td>
                <td>{command.title}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="muted">
        Conflicting registrations are rejected at registration time; the later registration loses
        its binding and stays reachable from the command palette.
      </p>
      {rejected.map((command) => (
        <p key={command.id} className="error" data-testid="rejected-binding">
          &quot;{command.title}&quot; lost its &quot;{command.shortcut}&quot; binding to &quot;
          {commands[command.shortcutRejectedBy ?? '']?.title ?? command.shortcutRejectedBy}&quot;.
        </p>
      ))}
    </Panel>
  );
}

function formatBinding(command: { shortcut?: string }): string {
  if (!command.shortcut) {
    return '';
  }
  try {
    return formatShortcut(parseShortcut(command.shortcut));
  } catch {
    return command.shortcut;
  }
}
