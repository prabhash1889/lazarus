import type { MethodVersion } from './types.ts';

import { METHODS } from './methods.ts';
import { snapshotManifest } from './methods.ts';

/**
 * The frozen released-floor method set: every method name and minimum
 * version that every supported Host must keep serving, forever. Names may
 * never be removed from this list; versions here only move forward when a
 * coordinated fleet-wide upgrade makes a higher floor true everywhere.
 */
export const RELEASED_FLOOR: ReadonlyMap<string, MethodVersion> = new Map([
  ['system.health', { major: 1, minor: 0 }],
  ['system.getInfo', { major: 1, minor: 0 }],
  ['system.subscribeEvents', { major: 1, minor: 0 }],
  ['workspace.list', { major: 1, minor: 0 }],
  ['task.list', { major: 1, minor: 0 }],
  ['process.start', { major: 1, minor: 0 }],
  ['process.stop', { major: 1, minor: 0 }],
  ['process.list', { major: 1, minor: 0 }],
  ['process.output', { major: 1, minor: 0 }],
  ['process.resume', { major: 1, minor: 0 }],
]);

/** Floor methods missing from (or below floor in) the current registry. */
export function releasedFloorGaps(): string[] {
  const gaps: string[] = [];
  for (const [name, floor] of RELEASED_FLOOR) {
    const method = METHODS.find((m) => m.name === name);
    if (method === undefined) {
      gaps.push(name);
      continue;
    }
    if (method.version.major !== floor.major || method.version.minor < floor.minor) {
      gaps.push(`${name} (below floor ${floor.major}.${floor.minor})`);
    }
  }
  return gaps;
}

/**
 * The manifest every peer exchange starts from: the floor, rendered in the
 * same snapshot form used on the wire and by codegen.
 */
export function releasedFloorSnapshot() {
  const full = snapshotManifest();
  return {
    ...full,
    methods: full.methods.filter((m) => RELEASED_FLOOR.has(m.name)),
  };
}
