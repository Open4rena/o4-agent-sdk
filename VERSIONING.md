# Versioning

The o4 Agent SDK versions independently of the o4 CLI/runtime.

The SDK and the runtime are separate artifacts with separate release cycles.
The SDK's version (this workspace, currently `0.4.1`) tracks only the SDK's own
public API and the wire protocol. It is **not** tied to o4's version number,
which moves on a much faster, patch-per-fix cadence.

## What the SDK is compatible with

Rather than matching o4's version, the SDK declares which **runtime** it works
with in [`compatibility.toml`](compatibility.toml):

```toml
runtime_commit = "<o4 repo commit providing o4-agent-runtime>"
runtime_version = "0.3.43"
protocol_version = 1
runtime_version_requirement = ">=0.0.57, <1.0.0"
```

Compatibility rests on three axes, all enforced at connection time:

1. **Protocol version** -- the wire format. Both sides must agree (v1 today).
2. **Capabilities** -- any capability the application requires must be
   advertised by the runtime.
3. **Runtime version** -- the runtime's semver must satisfy
   `runtime_version_requirement`.

`runtime_version` and `runtime_commit` record the exact runtime this SDK was
built and verified against. `runtime_version_requirement` is compiled into the
SDK and checked during the handshake, so a runtime outside the accepted range is
rejected before any command is sent.

## When to bump

- **Bump the SDK version** when its public API or the wire protocol changes.
- **Bump the pinned runtime** (`runtime_version` / `runtime_commit`) only after
  verifying the SDK against a newer `o4-agent-runtime`, and widen or narrow
  `runtime_version_requirement` deliberately at the same time.

Because the two version lines are independent, an o4 patch release that does not
touch the runtime's protocol or behavior requires no SDK change.
