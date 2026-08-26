import { describe, expect, it } from 'vitest';

import { formatUptimeSeconds, uptimeSecondsFrom } from './uptime';

describe('formatUptimeSeconds', () => {
  it('renders human-readable spans without zero-padding noise', () => {
    expect(formatUptimeSeconds(0)).toBe('0s');
    expect(formatUptimeSeconds(59)).toBe('59s');
    expect(formatUptimeSeconds(60)).toBe('1m 0s');
    expect(formatUptimeSeconds(3_725)).toBe('1h 2m 5s');
    expect(formatUptimeSeconds(93_000)).toBe('1d 1h 50m');
  });

  it('clamps negative inputs', () => {
    expect(formatUptimeSeconds(-10)).toBe('0s');
  });
});

describe('uptimeSecondsFrom', () => {
  it('derives elapsed seconds from epoch stamps', () => {
    const now = 1_756_100_000_000;
    expect(uptimeSecondsFrom(now - 65_000, now)).toBe(65);
    expect(uptimeSecondsFrom(now, now)).toBe(0);
  });

  it('returns null without a stamp or with a pre-epoch value', () => {
    expect(uptimeSecondsFrom(null, 1_756_100_000_000)).toBeNull();
    expect(uptimeSecondsFrom(0, 1_756_100_000_000)).toBeNull();
  });
});
