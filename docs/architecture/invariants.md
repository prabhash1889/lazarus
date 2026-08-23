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

- INV-8: `lazarus-hostd` owns PTYs and agent child processes directly (process groups / Windows Job Objects, bounded replay, resource accounting). No separate runner daemon exists; no other component supervises agent processes.
- INV-9: After an unexpected Host death, Host-owned child processes are marked interrupted. They may be resumed/restarted only where the provider supports it, and interruption is always visible to the user.
- INV-10: Desktop/CLI communicate with the Host only through the versioned Lazarus Protocol with per-method `{major, minor}` manifests, never via shared in-process state and never via a single global protocol version.

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

- INV-22: RPC versions (per-method `{major, minor}`), persistence record versions, SQLite migration numbers, artifact format versions, and package semver evolve independently; none may be used as the currency of another. Compatibility rules: `docs/protocol/compatibility.md`.
- INV-23: The protocol contract source of truth is the TypeScript/Zod protocol package. JSON Schema fingerprints and generated Rust bindings must match it; hand-maintained parallel contracts are forbidden. Additive changes are backward-compatible within a method major version; unknown fields are ignored.
- INV-24: Streaming recovery uses restart tombstones, resubscription, and authoritative snapshots - not a universal replay envelope or replay-window guarantee. Write RPCs remain idempotent where needed.
- INV-25: Distributed features are not pre-built. Only small documented seams (stable IDs, versioned formats/protocols) exist to avoid future lock-in.

## 9. Autonomy bounds

- INV-26: All autonomous execution runs under explicit policy: budgets (time/tokens/cost/tool calls), concurrency caps, allowed providers/directories/network/shell commands, and human-approval gates where configured.
- INV-27: "The agent says it is done" is never completion evidence. Completion requires deterministic gates and/or independent verification evidence.
- INV-28: Users can always see why work is running, blocked, retried, or complete.

## 10. Product surface parity

- INV-29: One Task aggregate owns chats, terminal agents, artifacts, workspace folders, terminals, files/diffs, and agent lineage. "Epic" is UI/product vocabulary for the same aggregate, never a second engine.
- INV-30: Exactly four artifact kinds are wire-level built-ins: Spec, Ticket, Story, Review. ADRs, plans, decision logs, and walkthroughs are templates/conventions stored as Specs or Reviews until a real need justifies another kind.
- INV-31: Planning, review, critique, and walkthrough behavior ships as versioned skills/workflows on the Task surface, not as separate top-level application modes. Autopilot/DAG execution is deferred work, not a current primitive.
- INV-32: The Host binds to loopback only in local mode. Host, CLI, and Desktop share `LAZARUS_LOCAL_TOKEN`; every request authenticates with `Authorization: Bearer` before manifest negotiation or handler logic.
