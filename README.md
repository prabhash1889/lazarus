# Lazarus

Lazarus is a local-first, multi-agent, spec-driven software engineering control plane. It orchestrates coding agents (Claude Code, Codex, OpenCode, generic CLIs, and model APIs) on your own machine: gathering repository context, clarifying intent, producing durable specs and tickets, routing work to agents in isolated Git worktrees or containers, verifying results with deterministic gates plus independent AI review, and preserving task knowledge locally.

There is no Lazarus cloud backend. Everything - Desktop, local Host (`lazarus-hostd`), process supervisor (`lazarus-runnerd`), SQLite persistence, Git/worktree engine, verification, permissions, and audit history - runs locally. The product works fully without a Lazarus account or server.

## Core loop

```text
Intent -> gather local context -> clarify -> durable specs/artifacts
      -> decompose into executable work -> route to configured agents
      -> run write agents in isolated worktrees/containers
      -> observe tools/files/tests/diffs
      -> verify against requirements + deterministic gates
      -> repair within explicit limits
      -> human approval / Git integration / optional PR
      -> preserve task knowledge locally
```

## Status

Phase 0 (product contract and engineering foundations) per `LAZARUS_INITIAL_PLAN.md`: foundational desktop/host/runner/CLI shells plus the documentation and policy set under `docs/`.

## Development

Install Node 24.18, pnpm 9.15.9, Rust stable 1.96 or newer, and the platform prerequisites for Tauri 2. Then build every Phase 0 shell with one command:

```sh
pnpm bootstrap
```

Run the same format, lint, typecheck, build, and test gates as CI with:

```sh
pnpm run ci
```

## Planned components

| Component         | Role                                                                                  |
| ----------------- | ------------------------------------------------------------------------------------- |
| Tauri Desktop     | UI shell; owns no durable state                                                       |
| `lazarus-hostd`   | Local source of truth: tasks, agents, artifacts, workflows, verification, permissions |
| `lazarus-runnerd` | Small process supervisor: PTYs, agent child processes, reconciliation                 |
| Lazarus CLI       | Full core workflow parity without the GUI                                             |
| SQLite (WAL)      | Local persistence; append-only events + materialized views                            |
| Provider packs    | Versioned adapters for CLI and API-backed agents                                      |

## Documentation

- `docs/product/domain-glossary.md` - canonical domain terms
- `docs/architecture/invariants.md` - non-negotiable system invariants
- `docs/adr/0001-architecture.md` - foundational architecture decisions
- `docs/security/threat-model.md`, `docs/security/execution-isolation.md`, `docs/security/trust-classes.md`, `docs/security/privacy-principles.md`
- `docs/protocol/compatibility.md` - versioning rules for Desktop/CLI/Host/runner

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security reports: see [SECURITY.md](SECURITY.md).

## License

Apache License 2.0. See [LICENSE](LICENSE).
