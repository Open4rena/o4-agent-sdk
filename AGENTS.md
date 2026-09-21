# AGENTS.md

## Project Overview

The o4 Agent SDK is a standalone async Rust library for controlling the real
`o4-agent-runtime` process over a versioned, line-delimited JSON (JSONL)
protocol. It is a **control plane**: it does not implement an agent loop, call
model providers, or bundle the runtime. Applications link the SDK; the SDK
spawns and drives the runtime binary.

This repository is **private**. Keep it private until an explicit decision is
made to publish it (see "Going public" below).

## Architecture

Cargo workspace with two crates:

| Crate | Role |
|---|---|
| `o4-agent-protocol` | Serde wire types and protocol v1 compatibility constants. Shared **byte-identical** with the copy in the o4 repository. |
| `o4-agent-sdk` | Public async API: `Client`, `Session`, `RunHandle`, the abstract `Transport` trait, and the local subprocess transport `LocalRuntime`. |

```text
application
    |
Client / Session / RunHandle
    |  request IDs + one stdout reader/router
Transport (ordinary framed byte streams; no terminal/PTY API)
    |
LocalRuntime: piped stdin/stdout/stderr
    |
o4-agent-runtime (owns the agent loop and durable state)
```

## Invariants -- do not break these

These are the load-bearing design decisions. Changing one is a deliberate,
reviewed act, not a side effect of other work.

1. **No agent loop in the SDK.** The SDK sends commands and streams events. All
   agent behavior lives in the runtime.
2. **No sshable (or PTY/shell) dependency.** `Transport` exposes only
   `send_line`, `receive_line`, graceful `close`, best-effort `abort`, and
   bounded diagnostics. A future remote/SSH adapter implements this trait over
   plain byte streams. Do not add terminal, PTY, or process assumptions to the
   transport contract.
3. **Protocol types stay byte-identical with o4.** The canonical
   `o4-agent-protocol/src/lib.rs` lives in the o4 repository. If you change it
   here, the same change must land there; verify with `cmp`.
4. **Additive-only wire changes within v1.** Protocol structs do **not** use
   blanket `deny_unknown_fields`, so new optional fields don't break older
   clients. Unknown event tags must keep deserializing to `Event::Unknown`
   (preserving fields). Unknown *command* tags remain hard errors -- the SDK
   only sends commands it understands.
5. **Only stdout is protocol.** Never parse human output. Any non-JSON stdout
   line is a fatal protocol error.
6. **Fail closed.** Path validation, capability checks, and version negotiation
   reject on any doubt. The runtime is the final enforcement boundary, but the
   SDK must never silently weaken a policy, skip a check, or downgrade an error
   to a warning.

## Versioning and compatibility

The SDK versions **independently** of o4. Read and follow
[VERSIONING.md](VERSIONING.md) before touching versions.

- Compatibility with a runtime is declared in `compatibility.toml`
  (`runtime_version`, `runtime_commit`, `protocol_version`,
  `runtime_version_requirement`) and enforced at handshake.
- The default requirement `>=0.0.57, <0.3.0` is compiled in as
  `DEFAULT_RUNTIME_VERSION_REQUIREMENT`. It must equal
  `runtime_version_requirement` in `compatibility.toml`; nothing generates one
  from the other, so `compatibility_pin_matches_compiled_default` guards it.
- Bump the SDK version only on public-API or protocol change. Re-pin the
  runtime only after verifying against a newer `o4-agent-runtime`.

## Building and testing

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

- The integration suite uses an in-memory fake JSONL runtime and needs **no**
  provider API keys.
- End-to-end tests exercise the real runtime binary; point them at it with
  `O4_AGENT_RUNTIME_BIN=/path/to/o4-agent-runtime`. These must pass (not skip)
  before any release.

Run all three gates before committing code changes. Clippy runs with
`-D warnings` -- do not introduce new warnings.

## Commit conventions

- Commits are authored as `Habib0x0 <Habib0x0@users.noreply.github.com>` (the
  GitHub-linked noreply). This is set in repo-local config -- verify with
  `git config user.name` / `user.email` before the first commit on any fresh
  clone, and never let git fall back to a machine/hostname identity.
- Keep the `Co-Authored-By: o4 <noreply@open4rena.com>` trailer on commits.
- Do not commit or push unless the user explicitly asks. Never force-push
  without explicit approval.
- Conventional-commit style subjects (e.g. `feat(sdk): ...`, `fix(sdk): ...`,
  `docs(sdk): ...`).

## Security rules

- Never commit secrets, keys, or credentials. MCP stdio args/env and SSE
  URL/header values may carry tokens -- keep them redacted from `Debug` and out
  of error strings.
- `RuntimeDiagnostics` (captured runtime stderr) stays redacted in
  `Debug`/`Display`; `as_str()` is a sensitive, explicitly-secured operation.
- Public errors must not embed MCP or policy payloads.
- The runtime is spawned directly (no shell, no PTY) with piped stdio; stderr
  is drained into a bounded (64 KiB default) tail buffer.

## Going public

This repo is currently private. If it is ever prepared for public release:

- The versioning **policy** and `runtime_version_requirement` are safe to keep
  public -- users need them.
- Reconsider `runtime_commit` in `compatibility.toml`: it pins a commit of the
  private o4 repository. Decide whether o4 becomes public too, or the runtime
  ships as a standalone artifact/crate, before exposing that pin.
- Confirm the runtime distribution story first -- a public SDK that cannot
  obtain a compatible `o4-agent-runtime` is unusable.

Do not change repository visibility yourself; that is a user decision.
