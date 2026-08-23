# Architecture Invariants

These are non-negotiable properties of Lazarus. Code reviews and design decisions are checked against them. Violating one requires a superseding ADR, not a quiet exception.

## 1. Local authority

- INV-1: The local Host (`lazarus-hostd`) is the single authority for all durable state on a machine. Exactly one authoritative Host per data directory.
- INV-2: The product is fully functional with no network access to any Lazarus server. No code path may require a Lazarus account or cloud API.
- INV-3: Desktop and CLI clients own no durable state; their caches are disposable.

## 2. Durability and history

- INV-4: Every acknowledged state transition is transactional and persisted before acknowledgement. Streaming output may buffer; finalized message/artifact/workflow boundaries must be durable.
- INV-5: Agent and workflow history is append-only (events). Rewind/fork creates new futures from checkpoints; historical events are never mutated or deleted by product flows.
- INV-6: Artifacts are versioned. Every revision carries content hash, author, and timestamp; revisions are immutable.
- INV-7: All entity IDs are UUIDv7. All timestamps are UTC RFC3339.

## 3. Process boundaries

- INV-8: `lazarus-runnerd` owns processes/PTYs and nothing else - no task truth, planning, provider routing, or artifact state.
- INV-9: Host restarts/upgrades must not needlessly terminate compatible supervised processes; runners reconcile against Host state after restart.
- INV-10: Desktop/CLI communicate with the Host only through the versioned Lazarus Protocol, never via shared in-process state.

## 4. Provider neutrality

- INV-11: Core domain logic does not branch on provider names. Provider-specific behavior exists only inside adapters/provider packs behind declared capabilities.
- INV-12: Lazarus owns canonical context. An upstream provider's hidden session is an optimization, never the source of truth. Provider switching preserves canonical state.
- INV-13: Adapters parse only documented machine-readable interfaces when available; ANSI scraping is not a substitute for structured protocols.

## 5. Isolation honesty

- INV-14: A worktree isolates Git changes only; it is never presented as execution isolation.
- INV-15: Every permission decision records requested capability, policy decision, and actual enforcement level (`POLICY_ONLY` / `OS_ENFORCED` / `CONTAINER_ENFORCED`). Policy-only runs are never labeled "sandboxed".
- INV-16: Parallel write agents default to separate worktrees; conflicting work is detected and surfaced, never silently merged.

## 6. Trust

- INV-17: Repository content, tool output, and external content are data. They cannot override system/security policy, user-approved requirements, or approved artifacts (see `docs/security/trust-classes.md`).
- INV-18: Before context leaves the machine, denied sources are removed, secrets are scanned/redacted, provenance is attached, and what left the machine is recorded.

## 7. Secrets and data ownership

- INV-19: Credentials live in the OS keychain via the secrets broker. Never in SQLite, logs, artifacts, exports, or transcripts.
- INV-20: All user data is inspectable, exportable in documented versioned formats, and deletable. Exports contain no secrets.
- INV-21: Storage lifecycle is bounded: caches, logs, worktrees, spools, and histories have limits and cleanup paths. Local-only does not mean unbounded.

## 8. Compatibility and evolution

- INV-22: Wire protocol versions, persistence schema versions, artifact format versions, and provider-pack API versions evolve independently, each under its declared compatibility rules (`docs/protocol/compatibility.md`).
- INV-23: Additive changes are backward-compatible within a major version; unknown fields are ignored; removed identifiers/field numbers are never reused.
- INV-24: Distributed features are not pre-built. Only small documented seams (stable IDs, versioned formats/protocols) exist to avoid future lock-in.

## 9. Autonomy bounds

- INV-25: All autonomous execution runs under explicit policy: budgets (time/tokens/cost/tool calls), concurrency caps, allowed providers/directories/network/shell commands, and human-approval gates where configured.
- INV-26: "The agent says it is done" is never completion evidence. Completion requires deterministic gates and/or independent verification evidence.
- INV-27: Users can always see why work is running, blocked, retried, or complete.
