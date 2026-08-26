# Desktop UI Shell Conventions

Phase 3.1 established the desktop shell primitives. Every later feature screen builds on these
conventions; deviations need a new ADR.

## Routing

- Routes are declared in `apps/desktop/src/app/router.tsx` using TanStack Router code-based routes.
- Screen components live in `src/screens/` and are loaded through `React.lazy` so each screen is
  its own chunk.
- Every route declares `errorComponent` and `pendingComponent`; the router also declares
  `defaultErrorComponent` and `defaultPendingComponent`.
- The hash history is used because Tauri serves the frontend from a custom protocol where browser
  history fallback is not guaranteed.
- Navigation from the native menu arrives over the `shell://navigate` Tauri event and must pass the
  `isNavTarget` whitelist in `src/lib/shell-events.ts` before it reaches the router.

## Error handling layers

1. Route-level failures render `RouteErrorFallback` with recovery actions (try again / reload).
2. Uncaught render errors inside the shell are caught by the `ErrorBoundary` in the root layout so
   the window never blanks.
3. Uncaught render errors that take down the whole router are caught by the outer `ErrorBoundary`
   in `AppRoot`.
4. Failed queries surface through the toast subsystem after retries are exhausted.

## Data fetching

- All host communication goes through TanStack Query with the shared client from
  `src/app/query-client.ts` (retry with capped exponential backoff, 15s staleness, refetch on
  focus).
- Never call `invokeCommand` outside a query or mutation.
- Mutations invalidate the affected query keys in `onSettled` so the UI reflects host state.

## State management: Zustand for transient UI state only

- Zustand stores live in `src/state/` and hold **transient UI state only**: toasts, open/closed
  panels, ephemeral selections.
- Durable state belongs to the Host (SQLite) and must be created and read through the Lazarus
  Protocol, never mirrored into a client store as the source of truth.
- Desktop caches are disposable by design; losing a Zustand store must never lose user data.
- Access stores with selector hooks (`useToastStore((s) => s.toasts)`) so components subscribe to
  the narrowest slice.
- Do not persist Zustand stores to localStorage; the only allowed persistence is the theme choice
  in `ThemeProvider` because it must apply before the Host is reachable.

## Theme

- All colors, spacing, radii, typography, and shadows come from the CSS custom properties in
  `src/theme/tokens.css`. Do not hardcode pixel or hex values in component styles.
- Light is the default palette on `:root`; `[data-theme='dark']` overrides it. The resolved theme is
  applied to `document.documentElement.dataset.theme` by `ThemeProvider`.
- The theme choice defaults to the OS preference and persists to `localStorage` under
  `lazarus.theme`.
- Components must remain legible in both palettes; verify against both when adding UI.

## Radix primitives

- Interactive primitives come from Radix (`@radix-ui/react-slot`, `@radix-ui/react-dialog` today)
  and are wrapped in `src/components/` (`Button`, `Dialog`) with token-based styling.
- Use the wrappers instead of importing Radix directly in screens so focus management, ARIA
  attributes, and theming stay consistent.

## Accessibility (Phase 3.5)

- Composite widgets use roving tabindex from `src/lib/a11y/roving-tabindex.ts`: exactly one tab
  stop per widget, arrow keys move focus, activation stays manual (Enter/Space/click).
- Tab strips whose visible layout wraps each tab in a container keep ARIA ownership intact with a
  visually-hidden `role="tablist"` element that owns tabs through `aria-owns`; do not nest
  interactive controls inside tab buttons.
- Custom overlays (menus, drawers) trap and restore focus with `useFocusTrap` from
  `src/lib/a11y/focus-trap.ts`; Radix dialogs and the command palette rely on their own trapping.
- JS-driven motion must consult `useReducedMotion()`; CSS transitions are already neutralized
  under `prefers-reduced-motion: reduce`.
- Every text-bearing color pair must pass WCAG AA; `tokens.contrast.test.ts` enforces this against
  `tokens.css` in both palettes, so token edits fail CI when they break legibility.
- Screens must stay axe-clean: `src/a11y/phase3.a11y.test.tsx` scans every Phase 3 surface. Add a
  scan for any new screen or dialog.
- Heavy-component rendering status across WebView engines is tracked in
  `engine-rendering-matrix.md`; re-run the matrix before shipping features that depend on those
  components.

## Tauri shell contract

- The Rust shell owns: single-instance focus, the native menu, deep-link argv capture
  (`lazarus://`, validation deferred to Phase 20), and crash-safe window geometry persistence.
- Frontend code must tolerate the bridge being absent (plain browser dev) and degrade with a typed
  error instead of crashing.
