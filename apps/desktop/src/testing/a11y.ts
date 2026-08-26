import { run, type AxeResults } from 'axe-core';
import { expect } from 'vitest';

/**
 * Automated accessibility scanning helper (Phase 3.5). Wraps axe-core so
 * every Phase 3 screen test asserts the same contract with the same
 * configuration.
 */

/** Rules that cannot produce reliable results inside jsdom. */
const JSDOM_UNRELIABLE_RULES = [
  // jsdom performs no layout or style cascade resolution, so axe cannot
  // compute real contrast ratios; token contrast is enforced separately by
  // src/theme/tokens.contrast.test.ts against the actual palette values.
  'color-contrast',
];

export async function scanAccessibility(container: HTMLElement): Promise<AxeResults> {
  return run(container, {
    rules: Object.fromEntries(JSDOM_UNRELIABLE_RULES.map((rule) => [rule, { enabled: false }])),
  });
}

/** Asserts no violations; failure output lists each finding's target and help. */
export async function expectNoAxeViolations(container: HTMLElement): Promise<void> {
  const results = await scanAccessibility(container);
  const report = results.violations
    .map((violation) => {
      const targets = violation.nodes.map((node) => node.target.join(' ')).join('; ');
      return `${violation.id} (${violation.help}): ${targets}`;
    })
    .join('\n');
  expect(report).toBe('');
}
