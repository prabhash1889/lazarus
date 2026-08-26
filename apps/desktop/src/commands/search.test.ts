import { describe, expect, it } from 'vitest';

import { fuzzyMatch, recencyBoost, searchCommands } from './search';
import type { RegisteredCommand } from './types';
import type { UsageMap } from './search';

function makeCommand(overrides: Partial<RegisteredCommand> & { id: string }): RegisteredCommand {
  return {
    title: overrides.id,
    run: () => undefined,
    ...overrides,
  } as RegisteredCommand;
}

const NOW = 1_000_000;

describe('fuzzyMatch', () => {
  it('scores exact substring matches above scattered subsequences', () => {
    const substring = fuzzyMatch('host', 'Open Host status');
    const scattered = fuzzyMatch('host', 'ho-s-t-x');
    expect(substring).not.toBeNull();
    expect(scattered).not.toBeNull();
    expect(substring!).toBeGreaterThan(scattered!);
  });

  it('prefers matches at word boundaries and the start of text', () => {
    const start = fuzzyMatch('set', 'Settings');
    const middle = fuzzyMatch('set', 'offsets');
    expect(start!).toBeGreaterThan(middle!);
  });

  it('returns null when the query is not a subsequence', () => {
    expect(fuzzyMatch('zx', 'Settings')).toBeNull();
  });

  it('rewards consecutive characters', () => {
    const consecutive = fuzzyMatch('set', 'settings');
    const gapped = fuzzyMatch('set', 's e t x');
    expect(consecutive!).toBeGreaterThan(gapped!);
  });
});

describe('recencyBoost', () => {
  it('decays with age and adds a small frequency component', () => {
    const fresh = recencyBoost({ count: 3, lastUsedAt: NOW - 60_000 }, NOW, false);
    const dayOld = recencyBoost({ count: 3, lastUsedAt: NOW - 25 * 60 * 60_000 }, NOW, false);
    const none = recencyBoost(undefined, NOW, false);
    expect(fresh).toBeGreaterThan(dayOld);
    expect(dayOld).toBeGreaterThan(none);
    expect(none).toBe(0);
  });

  it('is capped when a query is present so popularity cannot beat relevance', () => {
    const boosted = recencyBoost({ count: 99, lastUsedAt: NOW }, NOW, true);
    expect(boosted).toBeLessThanOrEqual(15);
  });
});

describe('searchCommands', () => {
  const commands: RegisteredCommand[] = [
    makeCommand({ id: 'nav.home', title: 'Go to Home' }),
    makeCommand({ id: 'nav.host', title: 'Go to Host status', keywords: ['monitor'] }),
    makeCommand({ id: 'nav.settings', title: 'Go to Settings' }),
    makeCommand({ id: 'hidden', title: 'Unavailable thing', when: () => false }),
    makeCommand({ id: 'visible-when', title: 'Conditional thing', when: () => true }),
  ];

  it('filters by availability predicate', () => {
    const results = searchCommands('', commands, {}, NOW);
    const ids = results.map((entry) => entry.command.id);
    expect(ids).not.toContain('hidden');
    expect(ids).toContain('visible-when');
  });

  it('ranks keyword aliases alongside titles', () => {
    const results = searchCommands('monitor', commands, {}, NOW);
    expect(results[0]?.command.id).toBe('nav.host');
  });

  it('returns registration order for empty queries without usage', () => {
    const results = searchCommands('   ', commands, {}, NOW);
    expect(results.map((entry) => entry.command.id)).toEqual([
      'nav.home',
      'nav.host',
      'nav.settings',
      'visible-when',
    ]);
  });

  it('weights recent use ahead of registration order for empty queries', () => {
    const usage: UsageMap = { 'nav.settings': { count: 2, lastUsedAt: NOW - 30_000 } };
    const results = searchCommands('', commands, usage, NOW);
    expect(results[0]?.command.id).toBe('nav.settings');
  });

  it('keeps strong textual matches above weakly-recented commands', () => {
    const usage: UsageMap = { 'nav.home': { count: 9, lastUsedAt: NOW } };
    const results = searchCommands('settings', commands, usage, NOW);
    expect(results[0]?.command.id).toBe('nav.settings');
  });

  it('drops non-matching commands entirely', () => {
    const results = searchCommands('zzzz', commands, {}, NOW);
    expect(results).toHaveLength(0);
  });
});
