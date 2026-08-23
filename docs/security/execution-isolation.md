# Execution Isolation

Lazarus offers four run locations for agents. The central rule, repeated everywhere it matters:

**A worktree is not a sandbox.** Worktrees isolate Git changes. They provide zero operating-system isolation.

## Run locations

| Run location       | Prevents Git collision |           Filesystem isolation |    Network isolation | Secret isolation | Suitable for untrusted autonomous code |
| ------------------ | ---------------------: | -----------------------------: | -------------------: | ---------------: | -------------------------------------: |
| Local working tree |                     No |                             No |                   No |      Policy only |                                     No |
| Local worktree     |                    Yes |                             No |                   No |      Policy only |                                     No |
| OS sandbox profile |                    Yes | Partial/strong depending on OS |       Partial/strong |           Scoped |                                  Maybe |
| Container          |                    Yes | Strong if configured correctly | Strong if configured |           Scoped |                                    Yes |

## When each is appropriate

- **Local working tree:** only when the user explicitly selects it, exactly one write-capable agent is active, and the user understands changes are immediate and unisolated.
- **Worktree (default for write agents):** fast, good for parallel work and clean diffs; not a security boundary.
- **OS sandbox profile:** platform-specific restricted profiles (Linux namespaces/Landlock/seccomp, macOS controls); useful middle ground, guarantees vary by OS and must never be overstated in the UI.
- **Container:** strongest local isolation; required posture for untrusted autonomous execution. Docker first, Podman compatible.

## Enforcement levels

Every permission decision records three things:

```text
requested capability
policy decision
actual enforcement level
```

The UI must always distinguish:

- `POLICY_ONLY` - a permission engine rule; a misbehaving process inside the run location can violate it.
- `OS_ENFORCED` - the OS kernel rejects violations (namespace/seccomp/Job Object-level restrictions where applicable).
- `CONTAINER_ENFORCED` - the container boundary rejects violations.

**Never label a policy-only local run as "sandboxed."**

## Platform enforcement strategy

- **Linux:** containers first; optionally bubblewrap/namespaces + Landlock/seccomp profiles for restricted local runs.
- **macOS:** container/VM-backed isolation for strong guarantees; platform process controls for weaker local profiles.
- **Windows:** container/VM isolation for strong guarantees; Job Objects and process restrictions are for lifecycle/resource control, not a complete filesystem security boundary.

## Container profiles

Container execution is declared via profiles, for example:

```yaml
name: node-default
image: ghcr.io/lazarus/base-node:24
workspace_mount: rw
network: restricted
cpu: 4
memory: 8GiB
timeout: 2h
secrets:
  - github_token
```

Secrets are injected through the secrets broker at run time, scoped to the profile, and never baked into images or stored in container config files.

## Layering

Isolation composes: worktree (change isolation) + container (OS isolation) + permission profile (capability policy) + egress policy (network). Each layer is independent and auditable; the audit log records which layers were active for every run.
