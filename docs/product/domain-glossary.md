# Lazarus Domain Glossary

Canonical terms for the whole project. Code, protocol schemas, docs, and UI copy must use these terms consistently. Changes require a PR that updates all usages.

## Core entities

| Term                   | Definition                                                                                                                                                                                                     |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Host**               | The local Lazarus daemon (`lazarus-hostd`); the single source of truth for tasks, agents, artifacts, workflows, verification, and permissions on a machine. One authoritative Host per Lazarus data directory. |
| **Runner**             | The local process supervisor (`lazarus-runnerd`). Owns PTYs and agent child processes; owns no durable task/artifact truth. Reconciles against Host state after restart.                                       |
| **Desktop**            | The Tauri desktop application. A client of the Host; owns no durable state.                                                                                                                                    |
| **CLI**                | Command-line client with full core-workflow parity to the Desktop; also a Host client.                                                                                                                         |
| **Workspace**          | A registered local repository/directory the user opened. Identified by canonical path + repository fingerprint; carries VCS metadata and repo instructions.                                                    |
| **Task**               | A durable unit of user intent with a goal, requirements, mode, and status. The root object agents work on.                                                                                                     |
| **TaskWorkspace**      | Binding of a Task to a Workspace, including default run location and base ref.                                                                                                                                 |
| **Agent**              | A configured agent instance working on a Task, with an interface type (CHAT or TERMINAL), provider, model, permission profile, and optional parent (for child agents).                                         |
| **AgentRun**           | One bounded execution turn of an Agent: provider/model used, timing, status, stop reason, usage. An Agent may have many runs.                                                                                  |
| **AgentEvent**         | Append-only ledger entry recording agent history (messages, tool calls, file writes, checkpoints, usage). Immutable; the basis for replay and rewind.                                                          |
| **Checkpoint**         | A durable snapshot reference: canonical message cursor, artifact revisions, workspace/diff state. Enables resume, fork, and rewind without mutating history.                                                   |
| **Artifact**           | A durable typed document capturing intent or evidence (Spec, Ticket, Story, Review, ADR, Plan, Custom). Versioned via ArtifactRevisions; linked via ArtifactRelations.                                         |
| **Worktree**           | A separate Git working tree for change isolation. NOT a sandbox: isolates Git changes, not OS capabilities.                                                                                                    |
| **Workflow**           | A versioned DAG definition (graph + policy) compiled from product modes. Executed as a WorkflowRun of WorkflowNodeRuns with evidence.                                                                          |
| **Verification**       | The act of checking work against requirements and deterministic gates: VerificationRuns produce severity-ranked Findings with evidence.                                                                        |
| **Finding**            | A single severity-ranked, evidence-backed issue produced by verification or review.                                                                                                                            |
| **Provider**           | An external coding-agent CLI or model API that Lazarus drives through a provider pack.                                                                                                                         |
| **Provider pack**      | A versioned adapter package declaring a provider's binary, capabilities, compatibility range, and handoff templates.                                                                                           |
| **Permission profile** | A named policy granting/denying shell, network, filesystem, tool, and secret capabilities to agents. Decisions are recorded with enforcement level.                                                            |
| **Secrets broker**     | The Host service mediating OS-keychain access; secrets never enter SQLite, logs, or exports.                                                                                                                   |

## Modes

| Term               | Definition                                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------------------------------ |
| **Quick mode**     | Small-task flow: compact plan, one write agent, gates + review.                                              |
| **Plan mode**      | Normal single-PR feature/fix flow producing a detailed plan artifact.                                        |
| **Phases mode**    | Multi-step change flow with explicit phase checkpoints (DISCOVERY -> ... -> COMPLETE).                       |
| **Epic mode**      | Durable planning workspace: spec/story/ticket/review/ADR artifact hierarchy with traceability.               |
| **Review mode**    | Pipeline over a diff/branch/PR producing ranked, evidenced findings.                                         |
| **Autopilot mode** | Task compiled into a policy-governed DAG executed with hard controls (budgets, concurrency, approval gates). |

## Security and context terms

| Term                            | Definition                                                                                                                                                        |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Canonical context / message** | Lazarus-owned durable representation of a conversation; the source of truth for provider switching. Provider-private sessions are optimizations only.             |
| **Context ledger**              | The preserved record of user messages, assistant finals, tool summaries, decisions, and usage per run.                                                            |
| **Trust class**                 | The provenance label (e.g., USER_INSTRUCTION, SOURCE_CODE, WEB_OR_EXTERNAL_CONTENT) determining what content can influence. See `docs/security/trust-classes.md`. |
| **Run location**                | Where an agent executes: local working tree, worktree, OS sandbox profile, or container.                                                                          |
| **Enforcement level**           | What actually enforces a permission decision: `POLICY_ONLY`, `OS_ENFORCED`, `CONTAINER_ENFORCED`. Policy-only runs are never labeled "sandboxed".                 |
| **Lazarus Protocol**            | The versioned client-Host protocol (handshake, capabilities, unary + streaming RPCs). Compatibility rules in `docs/protocol/compatibility.md`.                    |

## Conventions

| Term               | Definition                                                                             |
| ------------------ | -------------------------------------------------------------------------------------- |
| **UUIDv7**         | Time-ordered UUID used for all stable entity IDs.                                      |
| **Canonical path** | OS-aware absolute normalized path (see `docs/architecture/invariants.md`).             |
| **Data directory** | `~/.lazarus/` and its fixed subdirectories; the root of all Lazarus-owned local state. |
