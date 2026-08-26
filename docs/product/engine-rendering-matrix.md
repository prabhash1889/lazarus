# Engine Rendering Matrix

Phase 3.5 exit-gate artifact. The heavy components that later phases depend on are prototyped and
exercised per WebView engine before feature work begins, so engine-specific defects surface now
instead of after feature screens exist.

- Prototype surface: the `/engine-matrix` route (also reachable from the command palette via
  "Open engine rendering matrix").
- Prototypes: `apps/desktop/src/components/engine-matrix/` - `TerminalPrototype` (xterm.js),
  `DiffPrototype` (unified diff renderer), `EditorPrototype` (CodeMirror 6).
- Automated coverage: `src/components/engine-matrix/prototypes.test.tsx` mounts each prototype,
  verifies it initializes, and (for the editor) types into it. The axe scans in
  `src/a11y/phase3.a11y.test.tsx` cover the matrix screen itself.

## How to validate on an engine

1. Build or download the debug package for the platform (the `desktop` workflow produces Windows,
   macOS, and Linux debug bundles on every push).
2. Launch the app, open the command palette (`Ctrl+K`), run "Open engine rendering matrix".
3. For each prototype below: confirm it renders, type/copy text into it, resize the window while
   it is visible, and toggle light/dark appearance.
4. Record the outcome row in the tracking table with date + app version.

## Tracking table

Statuses: `verified` - manually exercised on this engine; `automated-only` - covered by the jsdom
test suite but not yet interactively validated; `blocked` - cannot work without mitigation.

| Component | Engine | Status | Notes |
|---|---|---|---|
| xterm.js terminal | jsdom (CI) | verified | Mounts and reports ready; input echo wired. Canvas/WebGL renderers cannot be exercised in jsdom by design. |
| xterm.js terminal | WebView2 (Windows) | automated-only | Validate DOM renderer + typed input interactively; WebGL addon is intentionally not enabled yet. |
| xterm.js terminal | WKWebView (macOS) | automated-only | Validate after first macOS debug build is exercised. |
| xterm.js terminal | WebKitGTK (Linux) | automated-only | Validate font fallback and DPR scaling; known WebKitGTK caret issues may apply (see gaps). |
| Diff view | jsdom (CI) | verified | Hunk parsing, signed line classes, and horizontal overflow behavior tested. |
| Diff view | WebView2 (Windows) | automated-only | Check subpixel alignment of monospace columns at 125%/150% OS scale. |
| Diff view | WKWebView (macOS) | automated-only | Check `-webkit-overflow-scrolling` behavior for large hunks. |
| Diff view | WebKitGTK (Linux) | automated-only | Verify soft-wrap disabled rendering stays inside bounds. |
| CodeMirror editor | jsdom (CI) | verified | Mounts, accepts typed input, counts document updates (with Range geometry shims). |
| CodeMirror editor | WebView2 (Windows) | automated-only | Validate IME composition and bidirectional text before artifact editing ships. |
| CodeMirror editor | WKWebView (macOS) | automated-only | Known CM6/WKWebView measurement quirks; verify gutter alignment. |
| CodeMirror editor | WebKitGTK (Linux) | automated-only | Verify focus rings and keyboard navigation match the other engines. |

## Known gaps and mitigations

- **jsdom cannot measure layout.** Contrast is therefore enforced against real token values
  (`tokens.contrast.test.ts`) instead of rendered pixels, and `Range.getClientRects`,
  `Element.scrollTo`, and friends are shimmed in `src/testing/setup.ts` only for tests.
- **xterm canvas/WebGL renderers are deferred.** The prototype uses the default DOM renderer so
  the PTY-facing behavior lands first; GPU-accelerated addons stay out until Phase 20-era polish
  to avoid per-engine driver churn.
- **WebKitGTK caret and IME history.** WebKitGTK has historically shown caret-paint artifacts in
  contenteditable surfaces and slower composition handling. Mitigation: the artifact editor will
  ship behind the same prototype gate above, and the matrix must be re-run on WebKitGTK before
  Phase 7 (artifacts) exits.
- **`display: contents` avoided.** The tab strips keep their accessibility tree stable across
  engines by using visually-hidden `tablist` elements that own tabs through `aria-owns`, instead
  of relying on `display: contents`, which older WebKit builds mishandled.
- **Reduced-motion parity.** CSS transitions are neutralized under
  `prefers-reduced-motion: reduce`; any future JS-driven animation must consult
  `useReducedMotion()` so all three engines behave identically.
