# Security Policy

## Reporting a vulnerability

Do not open a public issue for security problems.

This new repository does not yet have a monitored private reporting channel. Maintainers must configure and publish one before the first public release. Until then, contact a maintainer privately and include:

- affected component (Desktop, `lazarus-hostd`, CLI, provider pack, docs/policy);
- platform and versions;
- reproduction steps or proof of concept;
- impact assessment.

Please allow a coordinated disclosure window before public discussion.

## Scope

In scope:

- anything that lets local code escape the declared isolation model (worktree/container boundaries);
- bypassing permission profiles, secrets broker, or trust-class rules;
- Host IPC authentication weaknesses (loopback socket/pipe, token handling);
- secret leakage into SQLite, logs, exports, transcripts, or provider payloads beyond policy;
- update/installer signature verification bypass;
- prompt-injection escalation: repository/tool/external content overriding user or security policy.

Out of scope (for now): attacks requiring the attacker to already control the user's OS account with equivalent privileges; social engineering of end users.

## Security posture summary

Lazarus is local-first and runs with the user's own privileges on their machine:

- Host listens only on loopback (Unix domain socket / Windows named pipe preferred); authenticated with per-install high-entropy tokens.
- Secrets live in the OS keychain, never in SQLite or portable exports.
- A worktree is NOT a sandbox - it isolates Git changes only. Strong isolation requires containers/OS sandbox enforcement, and the UI must label enforcement level honestly (`POLICY_ONLY` / `OS_ENFORCED` / `CONTAINER_ENFORCED`).
- Repository and external content is data; it cannot override system/security policy or user-approved requirements (see `docs/security/trust-classes.md`).

Details: `docs/security/threat-model.md`.
