/**
 * Uptime rendering for the Host status surface. Pure so the tick loop and
 * formatting are independently testable.
 */

export function formatUptimeSeconds(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const rest = seconds % 60;
  if (days > 0) {
    return `${days}d ${hours}h ${minutes}m`;
  }
  if (hours > 0) {
    return `${hours}h ${minutes}m ${rest}s`;
  }
  if (minutes > 0) {
    return `${minutes}m ${rest}s`;
  }
  return `${rest}s`;
}

export function uptimeSecondsFrom(
  startedAtUnixMs: number | null,
  nowUnixMs: number,
): number | null {
  if (startedAtUnixMs === null || startedAtUnixMs <= 0) {
    return null;
  }
  return Math.max(0, Math.floor(nowUnixMs / 1000 - Math.floor(startedAtUnixMs / 1000)));
}
