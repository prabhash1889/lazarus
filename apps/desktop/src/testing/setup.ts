/**
 * Shared jsdom polyfills for the desktop test suite. The heavy engine
 * prototypes (xterm.js, CodeMirror 6) touch layout APIs that jsdom does
 * not implement; these minimal shims keep those components exercisable.
 */

import { beforeEach } from 'vitest';

beforeEach(() => {
  // CodeMirror measures collapsed ranges; jsdom's Range lacks geometry.
  const proto = Range.prototype as unknown as Record<string, unknown>;
  if (typeof proto.getClientRects !== 'function') {
    proto.getClientRects = (): { length: number; item: () => null } => ({
      length: 0,
      item: () => null,
    });
  }
  if (typeof proto.getBoundingClientRect !== 'function') {
    proto.getBoundingClientRect = (): DOMRect =>
      ({
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        width: 0,
        height: 0,
        toJSON: () => undefined,
      }) as DOMRect;
  }
});

// xterm.js probes scroll APIs on mount.
window.scrollTo = window.scrollTo ?? (() => undefined);
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollTo !== 'function') {
  Element.prototype.scrollTo = (() => undefined) as Element['scrollTo'];
}
