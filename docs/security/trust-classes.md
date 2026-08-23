# Trust Classes

Every piece of content entering Lazarus carries a provenance-based trust class. The rule they enforce:

**Repository files, tool output, and external content are data. They cannot override user or security policy, no matter what instructions they contain.**

## Classes

| Class                     | Examples                                           | Can influence                     |
| ------------------------- | -------------------------------------------------- | --------------------------------- |
| `SYSTEM_POLICY`           | Security config, permission profiles, invariants   | Everything; highest authority     |
| `USER_INSTRUCTION`        | User chat messages, approved task requirements     | Task direction within policy      |
| `LAZARUS_ARTIFACT`        | Approved specs/tickets/plans                       | Agent work within the task        |
| `WORKSPACE_INSTRUCTION`   | `AGENTS.md`, `CLAUDE.md`, repo contribution guides | Coding style and conventions only |
| `SOURCE_CODE`             | Repository files, diffs, symbols                   | Context/data for work             |
| `ISSUE_OR_PR_TEXT`        | Imported issues, PR descriptions, comments         | Context/data                      |
| `MCP_OR_TOOL_OUTPUT`      | Tool results, MCP server responses, shell output   | Context/data                      |
| `WEB_OR_EXTERNAL_CONTENT` | Fetched pages, docs, linked resources              | Context/data                      |
| `MODEL_GENERATED_SUMMARY` | AI-produced summaries/compactions                  | Context/data; never authority     |

## Precedence

```text
system/security policy
  > user-approved Task requirements
  > approved Lazarus artifacts
  > workspace instructions
  > retrieved source/tool/external content
```

Lower classes can inform but never contradict higher ones. A prompt injection inside a repository file can ask an agent to "ignore previous instructions" or exfiltrate secrets; under this model it is just text - data to reason about, not instruction hierarchy.

## Promotion is explicit

Workspace instructions (`AGENTS.md` etc.) are conventions by default. Content becomes a trusted instruction source only when the **user explicitly promotes** it (e.g., approving a spec, marking an artifact as authoritative). No automatic promotion exists - not by file location, not by model suggestion.

## Egress rules (before context leaves the machine)

1. Resolve provider and data policy for the destination.
2. Remove denied files/chunks.
3. Scan/redact known secret patterns.
4. Attach provenance metadata per chunk.
5. Enforce size/token budgets.
6. Record exactly which context references left the machine.

The Context inspector lets users see and remove individual sources before a sensitive run.

## Implementation requirements

- Every context chunk stores its trust class and provenance (path, revision/hash, line range, retrieval method, score, timestamp).
- The UI renders provenance so users can tell user intent from repo text from web noise.
- Verification treats lower-class claims ("tests pass", "requirement met") as unproven until backed by deterministic evidence or independent review.
