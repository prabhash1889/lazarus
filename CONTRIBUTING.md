# Contributing to Lazarus

Thanks for your interest. This document covers the ground rules; the architecture context you need lives in `docs/`.

## Ground rules

- **License:** by contributing, you agree your contributions are licensed under Apache-2.0.
- **Conventional Commits:** use prefixes such as `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`. Example: `feat(host): single-instance lock`.
- **No cloud code:** do not add placeholder cloud services, sync queues, CRDT libraries, remote runners, or distributed abstractions. Distributed features are a future phase with documented seams only (`LAZARUS_INITIAL_PLAN.md` section 0.1).
- **Vertical slices:** reliability and verification come before broad autonomy.

## Before opening a PR

1. Read `docs/architecture/invariants.md`. Changes that violate an invariant will be rejected or require an ADR first.
2. If a change alters a documented decision, add a new ADR under `docs/adr/` using `docs/adr/template.md`; never silently contradict an existing ADR - supersede it explicitly.
3. Keep provider-specific logic inside provider adapters/packs. Core orchestration must not branch on provider names (`if provider == "..."` outside adapters is a bug).
4. Every state transition must be transactional and durable before acknowledgement. "Agent says done" is never completion evidence.
5. Never write secrets into SQLite, logs, exports, or artifacts. Secrets belong in the OS keychain via the secrets broker.

## Documentation changes

Docs are load-bearing here. Phase 0 freezes invariants; edits to:

- `docs/architecture/invariants.md`
- `docs/security/*`
- `docs/protocol/compatibility.md`

require a linked ADR or an explicit justification in the PR description.

## Style

- TypeScript: strict mode; Prettier + ESLint.
- Rust: rustfmt + clippy, no warnings on touched lines.
- Docs: concise, high-signal Markdown. No filler.

## Reporting issues

Include reproduction steps, OS/platform, relevant versions, and expected vs actual behavior. For security issues, do NOT open a public issue - see [SECURITY.md](SECURITY.md).
