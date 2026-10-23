# o4 Agent SDK for Rust

A standalone async Rust SDK for controlling the real `o4-agent-runtime` process over a versioned JSONL protocol. The SDK is a control plane: **it does not implement an agent loop**, call model providers, or depend on sshable.

- **Real runtime, not a reimplementation** -- drives the same agent loop, tools, sessions, MCP integration, and execution controls as the o4 CLI.
- **One-shot and persistent sessions** -- run a single prompt, or create durable sessions you can resume across process restarts.
- **Typed streaming events** -- consume message snapshots, tool calls, and results as a typed stream; unknown events stay forward-compatible.
- **Fail-closed execution policy** -- filesystem, process, and network controls enforced by the runtime.
- **Transport-agnostic** -- a plain byte-stream `Transport` trait with a built-in local subprocess transport; remote/SSH-ready by design.

This repository is an MVP workspace:

- [`o4-agent-protocol`](crates/o4-agent-protocol) contains serde wire types and v1 compatibility constants.
- [`o4-agent-sdk`](crates/o4-agent-sdk) contains the public async API, local subprocess transport, abstract transport contract, and event router.

The SDK versions independently of o4; see [VERSIONING.md](VERSIONING.md) for the compatibility policy.

## Installation

```toml
[dependencies]
o4-agent-sdk = { git = "https://github.com/Open4rena/o4-agent-sdk", tag = "v0.4.1" }
tokio = { version = "1", features = ["full"] }
```

Prerequisites:

- Rust 1.88+
- A compatible `o4-agent-runtime` binary. Every o4 release from 0.3.21 on publishes it as
  `o4-agent-runtime-<platform>.tar.gz` (with a `.sha256`) on
  [GitHub Releases](https://github.com/Open4rena/o4-releases), for `linux-x86_64`,
  `linux-arm64` and `macos-arm64`. The archive holds only the binary; point
  `LocalRuntime::new(...)` at it. See [Compatibility](#compatibility).

## Quick start

The shortest useful program -- one prompt, streamed message snapshots, final status:

```rust,no_run
use o4_agent_sdk::{Client, Event, LocalRuntime, SessionOptions};

#[tokio::main]
async fn main() -> o4_agent_sdk::Result<()> {
    let client = Client::connect(LocalRuntime::new("./o4-agent-runtime")).await?;

    let options = SessionOptions::new("claude-sonnet-4-6", "/tmp");
    let mut handle = client
        .create_session(options)
        .await?
        .run("List the files in the current directory")
        .await?;

    while let Some(envelope) = handle.next_event().await? {
        match &envelope.event {
            Event::MessageCompleted(message) => println!("message: {}", message.message),
            Event::ToolCompleted(tool) => println!("tool finished: {}", tool.tool_call),
            _ => {}
        }
    }

    println!("final: {:?}", handle.final_result().await?.status);
    client.shutdown().await?;
    Ok(())
}
```

## Examples

Complete programs in [`crates/o4-agent-sdk/examples/`](crates/o4-agent-sdk/examples). Run one from a checkout of this repository:

```sh
cargo run -p o4-agent-sdk --example live_progress -- /path/to/o4-agent-runtime claude-sonnet-4-6
```

| Example | What it shows |
|---|---|
| [`one_shot`](crates/o4-agent-sdk/examples/one_shot.rs) | One prompt with streamed output |
| [`persistent`](crates/o4-agent-sdk/examples/persistent.rs) | Several turns in one session, then closing and resuming it |
| [`live_progress`](crates/o4-agent-sdk/examples/live_progress.rs) | Live text and tool progress; press Enter to cancel |
| [`structured_report`](crates/o4-agent-sdk/examples/structured_report.rs) | A JSON report checked against a schema, with turn, token and time limits |
| [`resume_after_restart`](crates/o4-agent-sdk/examples/resume_after_restart.rs) | A conversation that continues after the program restarts |
| [`mcp_tools`](crates/o4-agent-sdk/examples/mcp_tools.rs) | Tools from an MCP server, including the bundled example server |
| [`human_approval`](crates/o4-agent-sdk/examples/human_approval.rs) | A person approves each file change before it happens (file tools only) |

Each takes the runtime path and a model name. The model `open4rena-smoke:open4rena-usage-smoke`, built into the runtime, needs no provider account: it gives fixed test replies, never real answers. To run every example that way, as CI does:

```sh
scripts/run-examples.sh /path/to/o4-agent-runtime
```

## Usage

### One-shot run

Create a session, run one prompt to completion, and unload it -- the SDK manages the session lifecycle for you:

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};

# #[tokio::main]
# async fn main() -> o4_agent_sdk::Result<()> {
let client = Client::connect(LocalRuntime::new("./o4-agent-runtime")).await?;

let result = client
    .run(
        "Summarize the workspace in one sentence",
        SessionOptions::new("claude-sonnet-4-6", "/tmp"),
    )
    .await?;

println!("status: {:?}", result.status);
# client.shutdown().await?;
# Ok(())
# }
```

### Persistent sessions

Create a durable session and run multiple prompts against it:

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};

# #[tokio::main]
# async fn main() -> o4_agent_sdk::Result<()> {
let client = Client::connect(LocalRuntime::new("./o4-agent-runtime")).await?;

let session = client
    .create_session(SessionOptions::new("claude-sonnet-4-6", "/tmp"))
    .await?;

let result = session.run("First prompt").await?.final_result().await?;
println!("status: {:?}", result.status);
# session.close().await?;
# client.shutdown().await?;
# Ok(())
# }
```

### Resume a session across processes

Durable sessions survive runtime restarts. A session created by one runtime process can be resumed by another:

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, ResumeOptions};

# #[tokio::main]
# async fn main() -> o4_agent_sdk::Result<()> {
let client = Client::connect(LocalRuntime::new("./o4-agent-runtime")).await?;
let session = client
    .resume_session("session-id", ResumeOptions::default())
    .await?;
# session.close().await?;
# client.shutdown().await?;
# Ok(())
# }
```

## Coding-agent skill

This repository includes one [o4 Agent SDK skill](skills/o4-agent-sdk/SKILL.md) with four bundled guides. It helps a coding agent build applications using the SDK's full current public API: runtime/provider setup, one-shot and durable sessions, structured results and limits, streaming/steering/cancellation, permissions, all three MCP transports, delegated agents, hosted-session observation, work dashboards, human approvals, custom transports, testing, and troubleshooting. Runtime-dependent features and wire-only operations are clearly distinguished.

The skill is guidance for the coding agent working on **your application**. It does not register a new tool inside an SDK session, install the runtime, or grant permissions.

### Install in o4

CI checks this installation with o4 0.3.21. Every published o4 release (0.2.78 and later) includes the plugin format it uses; older ones are untested.

```sh
mkdir -p "$HOME/.o4/plugins"
o4 plugin install Open4rena/o4-agent-sdk
o4 plugin list
```

This installs the bundle globally. Add `--project` to the install command for a project-local installation; review and trust that workspace first. Restart an existing o4 session, then open `/skills` or invoke it directly:

```text
/o4-agent-sdk Build a resumable Rust assistant with live progress and cancellation
/o4-agent-sdk Add bounded JSON reports to my CI worker
/o4-agent-sdk Connect my MCP server and ask before side-effecting tools run
```

The skill package is distributed from this repository's default branch. The Rust library dependency remains pinned to `v0.4.1`; that earlier tag does not contain the skill package. No provider keys or model calls are needed just to install or discover the skill. Installation does not automatically upgrade the SDK dependency in your application.

### Install the standalone skill

Alternatively, copy the **entire** `skills/o4-agent-sdk/` directory, including `references/`, into either:

- `<your-project>/.o4/skills/o4-agent-sdk/` for a trusted project, or
- `~/.o4/skills/o4-agent-sdk/` for your user account.

Keep one installed copy to avoid duplicate names; do not overwrite a customized skill without reviewing it. This path does not need a plugin manifest. For another coding agent that supports `SKILL.md`, use that agent's documented skill location and copy the same directory; portable files do not imply automatic discovery by every host. Bundled reference paths resolve from the installed skill directory, not the application's working directory.

### Guides and coverage

- [Sessions and runs](skills/o4-agent-sdk/references/sessions-and-runs.md) -- one-shot, multi-turn, cold resume, context, JSON schemas, limits, usage, streaming, steering, and cancellation.
- [Permissions, MCP, and sub-agents](skills/o4-agent-sdk/references/policy-mcp-and-subagents.md) -- policy semantics, stdio/SSE/Streamable HTTP configuration, credential handling, and runtime-mediated delegation.
- [Runtime, deployment, and transports](skills/o4-agent-sdk/references/transports-and-deployment.md) -- installation, compatibility, providers, CI/containers, custom connections, lifecycle, errors, and verification.
- [Hosted sessions, work, and approvals](skills/o4-agent-sdk/references/hosted-sessions-work-and-approvals.md) -- capability-gated observation, work snapshots/actions/watchers, and responding to human approval requests.

The skill's [public API coverage map](skills/o4-agent-sdk/SKILL.md#public-api-coverage-map) shows where each supported area is covered. Examples target SDK v0.4.1; verify the connected runtime's advertised capabilities before depending on optional features.

CI keeps the skill accurate: it compiles every Rust snippet in the guides, checks that their links still work after installation, fails when a public SDK name is missing from the guides, installs the package with a published o4, and runs the [examples](#examples) the skill points to against a published runtime.

## Documentation

- [Full SDK documentation](docs/agent-sdk.md) -- core concepts, execution policy, MCP, the wire protocol, and error handling.
- [VERSIONING.md](VERSIONING.md) -- compatibility policy and version guarantees.
- [AGENTS.md](AGENTS.md) -- contribution and design conventions for this repository.

## Compatibility

The SDK pins the exact runtime it was built and verified against in [`compatibility.toml`](compatibility.toml). At connection time it negotiates protocol version, capabilities, and runtime semver; any mismatch fails fast. See [VERSIONING.md](VERSIONING.md).

## Testing and development

The integration suite uses an in-memory fake JSONL runtime and needs no provider keys. End-to-end tests exercise the real runtime binary (set `O4_AGENT_RUNTIME_BIN`).

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo test --workspace --doc    # compiles the Rust code in the README, docs and skill
cargo clippy --workspace --all-targets -- -D warnings
scripts/run-examples.sh /path/to/o4-agent-runtime    # needs python3
```

[CI](.github/workflows/ci.yml) runs all of these on every push and pull request, using the published runtime named in [`compatibility.toml`](compatibility.toml).

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
