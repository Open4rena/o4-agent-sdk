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
    let client = Client::connect(LocalRuntime::new("o4-agent-runtime")).await?;

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

## Usage

### One-shot run

Create a session, run one prompt to completion, and unload it -- the SDK manages the session lifecycle for you:

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};

# #[tokio::main]
# async fn main() -> o4_agent_sdk::Result<()> {
let client = Client::connect(LocalRuntime::new("o4-agent-runtime")).await?;

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
let client = Client::connect(LocalRuntime::new("o4-agent-runtime")).await?;

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
let client = Client::connect(LocalRuntime::new("o4-agent-runtime")).await?;
let session = client
    .resume_session("session-id", ResumeOptions::default())
    .await?;
# session.close().await?;
# client.shutdown().await?;
# Ok(())
# }
```

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
cargo clippy --workspace --all-targets -- -D warnings
```

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
