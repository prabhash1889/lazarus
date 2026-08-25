import { declareBridge } from './bridges.ts';

/**
 * Production bridges for the current release. Every entry must reference a
 * registered method at its current version (enforced by
 * `assertRegistryInvariants`); the generator serializes this table into the
 * Rust bindings, where the Host executes it during negotiation.
 *
 * `task.list` 1.2 added the additive `servedAtUnixMs` response field; this
 * bridge keeps released 1.0 peers interoperable by stripping that field,
 * while 1.1 was never published and therefore stays undeclared (a peer
 * advertising 1.1 is refused rather than guessed at).
 */
declareBridge({
  method: 'task.list',
  older: { major: 1, minor: 0 },
  newer: { major: 1, minor: 2 },
  steps: [{ op: 'omitResponseFields', fields: ['servedAtUnixMs'] }],
});

/**
 * `system.getInfo` 1.1 added the additive `startedAtUnixMs` response field;
 * this bridge keeps released 1.0 peers interoperable by stripping that field.
 */
declareBridge({
  method: 'system.getInfo',
  older: { major: 1, minor: 0 },
  newer: { major: 1, minor: 1 },
  steps: [{ op: 'omitResponseFields', fields: ['startedAtUnixMs'] }],
});
