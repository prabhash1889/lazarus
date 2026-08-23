# 0001 - Foundational architecture: local-first, single host daemon, provider-neutral core

- Status: Accepted
- Date: 2026-08-23
- Deciders: Lazarus maintainers

## Context

Lazarus orchestrates multiple coding agents on a developer's machine. The initial build plan (`LAZARUS_INITIAL_PLAN.md`) requires that planning, execution, verification, and data ownership work with no cloud dependency, while leaving room for future distributed features without rewriting the local core. Several cross-cutting decisions must be frozen before any module is written, because every later component depends on them.

## Decision

The following are foundational and binding for all later phases:

1. **Local-first, no cloud.** The local Host (`lazarus-hostd`) is the single authority for Task, Agent, Artifact, Workflow, Verification, and integration state on a machine. Nothing requires a Lazarus account or server. No cloud services are built now; only small documented seams are preserved (stable UUIDv7 IDs, versioned protocols/exports, provider-neutral context).
2. **Single Host daemon.** `lazarus-hostd` owns durable state, orchestration, and PTY/agent child-process supervision (process groups / Windows Job Objects, bounded replay, interruption records). There is no separate runner daemon in the current build; splitting supervision out is a deferred option only if surviving Host restarts becomes a measured requirement. After an unexpected Host death, supervised processes are marked interrupted and may be resumed/restarted only where the provider supports it.
3. **Provider-neutral core.** Core domain logic never branches on provider names. Provider-specific behavior lives exclusively in versioned adapters/provider packs behind declared capabilities.
4. **Protocol boundaries are real.** Desktop and CLI communicate with the Host through a versioned protocol with per-method `{major, minor}` manifests (unary + streaming), not in-process state sharing and not one global protocol number. Phase 1.5 uses loopback Axum JSON/HTTP plus SSE; this transport may be superseded without changing the per-method contract. The contract source of truth is the TypeScript/Zod protocol package; JSON Schema fingerprints and Rust bindings are generated from it. Compatibility rules: `docs/protocol/compatibility.md`.
5. **Stable identity:** all entity IDs are UUIDv7 - lexicographically sortable by creation time, globally unique, sync-friendly if a future phase needs it.
6. **Time format:** UTC RFC3339 everywhere (persistence, protocol, exports, logs). No local-timezone timestamps in durable data.
7. **Paths:** canonical absolute paths are OS-aware (native separators and casing rules per OS), resolved to absolute form at registration time; portable data stores paths alongside a repo fingerprint rather than assuming one path layout.
8. **Local data directory:** all Lazarus-owned state lives under a single user-level data root (`~/.lazarus/`: `host/`, `state/`, `logs/`, `cache/`, `plugins/`, `auth/`, `backups/`). One Host instance is authoritative per data directory.
9. **Trust rules.** Repository files, tool output, and external content are data, not instructions; they cannot override system/security policy or user-approved requirements. Trust classes and precedence are defined in `docs/security/trust-classes.md`.

## Alternatives considered

- **Cloud-backed control plane from day one** (Traycer-style SaaS): rejected for this product phase - contradicts the local-first requirement, adds operational burden, and delays core value. Preserved seams make it addable later.
- **Separate runner daemon for process supervision:** rejected for the current build - a second lifecycle/protocol surface for no current user value. A single `hostd` owning PTYs matches the audited reference architecture; revisit only with a concrete PTY-survival requirement.
- **Global protocol version handshake / Protobuf contracts:** rejected - per-method `{major, minor}` negotiation from the TypeScript/Zod source of truth avoids lockstep upgrades and hand-maintained parallel Rust/Protobuf schemas.
- **Auto-increment integer IDs:** rejected - not stable across import/export/merge, no future-sync story.
- **Local-timezone timestamps / epoch integers only:** rejected - ambiguous across machines and tools; RFC3339 UTC is unambiguous and human-readable.
- **Per-provider orchestration logic:** rejected - unmanageable at 10+ providers; capabilities-declared adapters scale.

## Consequences

- Every module inherits these constraints; violations require superseding ADRs.
- Some future-cloud plumbing is intentionally deferred; the cost is small, documented seams now instead of rewrites later.
- Path handling code must be tested per-OS (Windows drive/case behavior differs from POSIX).
- The `~/.lazarus/` layout becomes a compatibility contract once released.
