# Privacy Principles

Lazarus is local-first. These principles govern how user data is handled and are binding for all components.

## Principles

1. **Local by default.** Tasks, artifacts, conversations, indexes, logs, and audit history live on the user's machine under `~/.lazarus/`. Nothing is sent to Lazarus-operated infrastructure - no such infrastructure exists in this product.
2. **Egress is explicit and visible.** Data leaves the machine only when the user configures providers/integrations or approves a run that sends context to them. Every egress records what left (which context references, to which provider, when) in the local audit log.
3. **Secrets never leave the keychain through data paths.** Credentials live only in the OS keychain via the secrets broker. They are excluded from SQLite, logs, transcripts, artifacts, exports, backups, and diagnostic bundles; known secret patterns are scanned/redacted before any context egress.
4. **No telemetry without opt-in.** No cloud-hosted telemetry exists. Any future diagnostic reporting will be off by default, inspectable before sending, and documented.
5. **Open, versioned formats.** All user-owned data is exportable in documented versioned Markdown/JSON formats. Users can inspect, back up, restore, and permanently delete everything Lazarus stores.
6. **Bounded retention.** Caches, indexes, spools, worktrees, and histories have explicit size/time limits with cleanup paths. Deletion of a Task/workspace removes its derived data within configured lifecycle rules.
7. **Third-party boundaries are honest.** When context goes to a model provider or integration (GitHub/GitLab/Jira/Linear), Lazarus shows which provider receives it and applies per-destination policy (denied files, redaction). Provider-side handling is governed by that provider's policy; Lazarus does not claim otherwise.
8. **Provenance travels with content.** Every stored/retrieved chunk carries its trust class and provenance, so users can audit why something was included in context.
9. **Diagnostics minimize exposure.** Diagnostic bundles are generated on demand, user-inspectable before sharing, and scrubbed of secrets by construction.

## What this means in practice

| Question                         | Answer                                                                                                        |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| Does Lazarus phone home?         | No. There is no Lazarus server to call.                                                                       |
| Where do my prompts and code go? | To model providers you configure, subject to trust-class filtering and redaction; recorded locally each time. |
| Can I use Lazarus fully offline? | Yes - except features that inherently require external services (model APIs, integrations).                   |
| How do I delete my data?         | Delete tasks/workspaces in-product, or remove `~/.lazarus/` state directories; exports contain no secrets.    |
| Who can read my local data?      | Processes running as your OS user. Host IPC is loopback-authenticated; see threat model T1.                   |
