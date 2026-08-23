# Threat Model (skeleton)

Status: living document, expanded per phase. Scope: the local-first Lazarus product only; no Lazarus cloud exists in this build.

## Assets

| Asset                                          | Location                       | Impact if compromised                                   |
| ---------------------------------------------- | ------------------------------ | ------------------------------------------------------- |
| Source code and Git history of user workspaces | User filesystem                | Code exfiltration or corruption                         |
| Secrets (tokens, keys)                         | OS keychain via secrets broker | Credential theft, account takeover of linked services   |
| Task/artifact/audit data                       | `~/.lazarus/state` + SQLite    | Privacy loss, tampered history                          |
| Host IPC channel                               | Loopback socket / named pipe   | Local privilege-style control of agents, files, secrets |
| Agent processes                                | Local worktree/container       | Arbitrary code execution with agent privileges          |
| Provider sessions/context packages             | In transit to model APIs       | Code/secret disclosure to third parties                 |
| Update artifacts                               | Downloaded binaries/packages   | Persistent malware via malicious update                 |

## Actors

- **User:** the machine owner; ultimate authority.
- **Host (`lazarus-hostd`):** runs with user privileges; brokers everything and owns PTYs/agent child processes.
- **Coding agents (LLM-driven):** semi-trusted workers that follow instructions from many sources, including untrusted ones.
- **Providers/model APIs:** external services receiving context we send.
- **Local malware / other user-level processes:** co-resident attackers with normal user privileges.

## Trust boundaries

```text
User <---- loopback JSON/HTTP + SSE (Bearer token) --------> Host
Host  ---- owns PTYs/processes --------------------------->  agent processes
Agent <-> repository files, tools, MCP servers   [untrusted content boundary]
Host  ---> provider APIs                          [egress boundary]
Desktop <- OS deep links/OAuth callbacks ->       [external entry points]
```

## Threats and mitigations

| #   | Threat                                                                            | Mitigations                                                                                                                                                                                                                                                     |
| --- | --------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1  | Another local process connects to Host IPC and drives agents/secrets              | Loopback-only bind (non-loopback local bind rejected); Host, CLI, and Desktop read `LAZARUS_LOCAL_TOKEN`; `Authorization: Bearer` is verified before manifest parsing or handler logic; every request carries `x-lazarus-manifest`; no pre-auth workspace paths |
| T2  | Malicious repo content injects instructions into agent context (prompt injection) | Trust classes: repo/tool/external content is data, cannot override policy/user requirements; provenance labels surfaced in UI                                                                                                                                   |
| T3  | Agent exfiltrates secrets via network or writes them into files/transcripts       | Egress policy separate from secret policy; secret redaction scans before egress; secrets broker denies direct reads to agents; audit log records what left                                                                                                      |
| T4  | Agent destructive actions outside its task scope                                  | Permission profiles (shell/network/filesystem/tools); run-location isolation; attributed diffs; human approval gates                                                                                                                                            |
| T5  | Policy-only run mistaken for a sandbox                                            | Enforcement level always displayed (`POLICY_ONLY`/`OS_ENFORCED`/`CONTAINER_ENFORCED`); never label policy runs "sandboxed"                                                                                                                                      |
| T6  | Malicious or compromised update                                                   | Signed manifests (Minisign/Cosign); SHA-256 verification; atomic versioned install; rollback on failed health check                                                                                                                                             |
| T7  | Secret leakage through SQLite/logs/exports/backups                                | Never store credentials outside keychain; export sanitization; log scrubbing; doctor checks                                                                                                                                                                     |
| T8  | Malicious MCP server / skill / plugin                                             | Signed packages where available; capability-scoped tool permissions; tool output is data (trust class), audited                                                                                                                                                 |
| T9  | Cross-worktree contamination between parallel agents                              | One worktree per write agent by default; path collision detection; path leases; no silent merges                                                                                                                                                                |
| T10 | Deep link / OAuth callback abuse on Desktop                                       | Origin/scheme validation, single-use tokens, short callback windows                                                                                                                                                                                             |
| T11 | Denial of service by runaway agents (CPU/RAM/disk/cost)                           | Hard budgets: runtime, tool calls, tokens/cost, repair loops; bounded spools, logs, indexes; resource accounting                                                                                                                                                |
| T12 | Tampering with audit/history to hide actions                                      | Append-only event ledger with checksums; integrity checks in `lazarus doctor`                                                                                                                                                                                   |

## Residual risks

- Co-resident malware with equal user privileges can do anything Lazarus can; Lazarus defends boundaries, not the user's own account.
- LLM-driven agents remain probabilistic; deterministic gates and human approval are the compensating controls, not afterthoughts.
- Provider-side handling of sent context is governed by provider policies, not ours; see privacy principles for what we disclose.
