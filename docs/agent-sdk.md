# o4 Agent SDK for Rust

The o4 Agent SDK is a standalone async Rust library for controlling o4 programmatically. It spawns and drives the real `o4-agent-runtime` process over a versioned, line-delimited JSON (JSONL) protocol -- the same agent loop, tools, session persistence, MCP integration, and execution controls that power the interactive CLI.

The SDK is a **control plane**, not an agent. It does not implement an agent loop, call model providers, or bundle the runtime. Your application links the SDK; the SDK spawns and talks to the `o4-agent-runtime` binary.

## When to use the SDK

Use the SDK when you want o4's agentic capabilities inside your own Rust application:

- Embed an agent in a server, daemon, or desktop app
- Run unattended batch/CI tasks with structured output
- Drive long-lived, resumable agent sessions across restarts
- Integrate o4 with your own transport (local subprocess today; remote/SSH-ready by design)

If you just want to chat or run one-shot prompts in a terminal, use the `o4` CLI instead.

## Architecture

```text
your application
      |
      v
 o4-agent-sdk (client library)
      |  typed async API: Client / Session / RunHandle
      |  JSONL v1 over a Transport
      v
 o4-agent-runtime (separate binary, from the o4 release downloads)
      |
      v
 real o4 agent loop + tools + sessions + MCP + policy enforcement
```

Two Rust crates make up the SDK workspace:

| Crate | Role |
|---|---|
| `o4-agent-protocol` | Serde wire types and protocol v1 compatibility constants. Shared verbatim with the runtime. |
| `o4-agent-sdk` | Public async API: `Client`, `Session`, `RunHandle`, local subprocess transport, and the abstract `Transport` trait. |

The runtime side (`o4-agent-runtime`) is published with each o4 release and is covered in [Agent runtime](#agent-runtime) below.

## Installation

Add the SDK as a dependency, referenced by git tag.

```toml
[dependencies]
o4-agent-sdk = { git = "https://github.com/Open4rena/o4-agent-sdk.git", tag = "v0.4.2" }
tokio = { version = "1", features = ["full"] }
```

You also need the `o4-agent-runtime` binary. Every o4 release from 0.3.21 on publishes it next to the CLI on [GitHub Releases](https://github.com/Open4rena/o4-releases), as `o4-agent-runtime-<platform>.tar.gz` with a `.sha256` checksum, for `linux-x86_64`, `linux-arm64` and `macos-arm64`. The Linux builds are static and run on any distribution. Pick a version inside the SDK's accepted range (see [Compatibility](#compatibility)):

```bash
VERSION=0.3.43
PLATFORM=linux-x86_64
BASE=https://github.com/Open4rena/o4-releases/releases/download/v${VERSION}
curl -fsSLO "${BASE}/o4-agent-runtime-${PLATFORM}.tar.gz"
curl -fsSLO "${BASE}/o4-agent-runtime-${PLATFORM}.tar.gz.sha256"
sha256sum -c "o4-agent-runtime-${PLATFORM}.tar.gz.sha256"   # shasum -a 256 -c on macOS
tar -xzf "o4-agent-runtime-${PLATFORM}.tar.gz"               # extracts ./o4-agent-runtime
```

The archive contains only the `o4-agent-runtime` binary. It takes no arguments and needs no `o4` CLI on the machine.

Point the SDK at the binary by passing its path to `LocalRuntime::new(...)`. The SDK's own end-to-end tests resolve it from the `O4_AGENT_RUNTIME_BIN` environment variable, which is a convenient convention to adopt in your code.

### Running the runtime on a server

The runtime reads two kinds of files, from two places:

| What | Where | Moved by `O4_AGENT_RUNTIME_DATA_DIR`? |
|---|---|---|
| Session state (durable sessions, session database) and `config.toml` | the data dir | yes (default `~/.o4`) |
| Provider setup: `providers.toml`, `catalog/`, `models.toml`, and saved keys in `settings.json` | `$HOME/.o4` | no |

So a worker with no operator home should set `HOME` to a directory it owns and mount its provider setup at `$HOME/.o4`. Setting `O4_AGENT_RUNTIME_DATA_DIR` alone moves the session state but leaves provider lookup at `$HOME/.o4`, and a model from a `providers.toml` placed only in the data dir is reported as `not registered`. Use `LocalRuntime::data_dir(...)` when you want session state somewhere other than `$HOME/.o4`; it sets `O4_AGENT_RUNTIME_DATA_DIR` for the child.

Nothing in `$HOME/.o4` is required for built-in providers: Anthropic, OpenAI and the other built-ins read their standard environment variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...). A custom OpenAI-compatible provider needs only `$HOME/.o4/providers.toml`; its `api_key_env` names the environment variable the runtime reads the key from on every request, so no credential has to be on disk. The runtime removes that variable from the environment of commands the agent runs, the same as it does for built-in provider keys.

Two things to know for containers:

- `providers.toml` is skipped, with a warning on stderr, if it is world-writable. Volume mounts and `COPY` can produce that permission; `chmod 644` fixes it.
- Providers that sign in through a browser (ChatGPT/Codex OAuth, Claude Code OAuth) cannot work headless. Use an API-key provider.

## Quick start

### One-shot run

The simplest path: connect, run a prompt to completion in a temporary session, get a final result. The SDK creates and closes the session for you.

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};

#[tokio::main]
async fn main() -> o4_agent_sdk::Result<()> {
    // Spawn o4-agent-runtime and negotiate compatibility.
    let runtime = std::env::var("O4_AGENT_RUNTIME_BIN").unwrap_or_else(|_| "o4-agent-runtime".into());
    let client = Client::connect(LocalRuntime::new(runtime)).await?;

    // Run one prompt in a fresh, temporary session.
    let options = SessionOptions::new("claude-sonnet-4-6", "/path/to/project");
    let result = client.run("Summarize this repository", options).await?;

    println!("status: {:?}", result.status);
    for message in &result.messages {
        println!("{message}");
    }

    client.shutdown().await?;
    Ok(())
}
```

### Persistent session

Create a durable session, run multiple prompts against it, and keep it across runs.

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};

#[tokio::main]
async fn main() -> o4_agent_sdk::Result<()> {
    let runtime = std::env::var("O4_AGENT_RUNTIME_BIN").unwrap_or_else(|_| "o4-agent-runtime".into());
    let client = Client::connect(LocalRuntime::new(runtime)).await?;

    let options = SessionOptions::new("claude-sonnet-4-6", "/path/to/project");
    let session = client.create_session(options).await?;
    let session_id = session.id().to_owned();

    let result = session.run("Add a health-check endpoint").await?.final_result().await?;
    println!("{:?}", result.status);

    // The session persists. Resume it later, even from a new process.
    let resumed = client
        .resume_session(session_id, Default::default())
        .await?;
    let result = resumed.run("Now add tests for it").await?.final_result().await?;
    println!("{:?}", result.status);

    client.shutdown().await?;
    Ok(())
}
```

## Core concepts

### Client

`Client` is the connection to one runtime process. It owns a single reader task and routes responses to the right caller by request ID, so clones of a `Client` are cheap and safe to share across tasks.

- `Client::connect(source)` -- open a transport, initialize the runtime, and negotiate protocol version, capabilities, and runtime version. Fails fast on any mismatch.
- `Client::connect_with_options(source, options)` -- same, with custom `ClientOptions`.
- `client.run(prompt, options)` -- one-shot run in a temporary session, returning the final `RunResult`.
- `client.create_session(options)` -- create a durable `Session`.
- `client.resume_session(id, options)` -- reattach to an existing durable session.
- `client.runtime_info()` -- negotiated runtime version and capabilities.
- `client.shutdown()` -- graceful runtime shutdown.

`ClientOptions` lets you set the client identity, override the runtime version requirement, require/advertise capabilities, and tune the handshake timeout.

### Session

A `Session` is a durable agent context bound to a model and a working directory.

- `session.id()` -- the session's stable ID.
- `session.run(prompt)` / `session.run_with_options(prompt, options)` -- start a run, returning a `RunHandle`.
- `session.close()` -- close the session on the runtime.

`SessionOptions::new(model, cwd)` is the minimum. You can also set a custom `session_id`, `system_prompt`, `reasoning` effort, an `execution_policy`, `mcp_server` entries, and a `context` (workspace/user/correlation IDs).

`ResumeOptions` accepts an optional `cwd` override and `context`.

Sessions are **durable**: they survive process restarts. A session created by one runtime process can be resumed by a different runtime process, which is the foundation for long-running and recoverable agents.

### RunHandle and streaming events

Starting a run returns a `RunHandle`, which is a `futures::Stream` of `EventEnvelope`s plus control methods.

- `handle.next_event()` -- await the next event.
- `handle.final_result()` -- consume the stream to completion and return a `RunResult`.
- `handle.cancel()` -- cancel the run.
- `handle.steer(instruction)` -- inject a mid-run steering instruction.
- `handle.known_run_id()` -- the run ID once the runtime has assigned it.

Events arrive as typed `Event` variants inside an `EventEnvelope` (which also carries sequence, timestamp, and optional session/run/context metadata). Unknown event types are preserved as `Event::Unknown`, so a newer runtime does not break an older SDK.

`RunOptions` controls a run: `output_schema` (structured JSON output), `max_turns`, `token_budget`, and `timeout_secs`. `RunResult` reports the final `status` (`RunStatus`), `messages`, `usage`, `turns`, and `tool_calls`.

### Execution policy

Every session can carry an `ExecutionPolicy` that the runtime enforces **fail-closed**:

```rust
use o4_agent_sdk::{ExecutionPolicy, SandboxMode};

let policy = ExecutionPolicy {
    // A non-empty list is the session's whole toolset: only these tools.
    allowed_tools: vec!["read".into(), "glob".into(), "grep".into(), "ls".into()],
    sandbox_mode: SandboxMode::WorkspaceWrite,
    allowed_domains: vec!["api.github.com".into()],
    ..ExecutionPolicy::default()
};
```

- `allowed_tools` -- empty: the sandbox mode's baseline tools. Non-empty: **only** the listed tools, replacing the baseline. Verified on runtime 0.3.21: a `WorkspaceWrite` session allowing only `read` cannot write files.
- `denied_tools` -- tools to remove. Naming a tool in both lists is an error.
- `sandbox_mode` -- `Disabled`, `ReadOnly` (default), or `WorkspaceWrite`.
- `sandbox_config` -- optional sandbox-specific configuration.
- `allowed_domains` -- network egress allowlist for fetch/search tools. Empty grants no network access.
- `interactive_approvals` -- make side-effecting tool calls wait for an answer from the application (see `Client::approvals`). Off by default.

The baseline under `ReadOnly` is the read-only file tools, internal helpers and the `agent` tool; `WorkspaceWrite` adds the file-writing tools; network tools join only when `allowed_domains` is set. Unknown or ambiguous tool names are rejected at session creation rather than silently weakening policy.

Every capability a session assembles must have an enforceable policy class. In a restricted profile the runtime refuses to create the session rather than exposing something it cannot enforce, and tool provenance is host-owned -- a plugin or MCP tool cannot acquire a built-in's privileges by reusing its name.

### Sub-agents

Hosted sessions get the `agent` tool from the same shared builder as the interactive TUI and `--print`, so an SDK session can delegate:

- **Depth** is capped at 3. The root session is depth 0, children exist at depths 1-3, and a depth-3 agent cannot spawn further. The cap is a fixed constant.
- **Inheritance:** a child runs under the parent's model and reasoning effort, API-key resolution, the filesystem policy derived from `sandbox_mode`/`sandbox_config`, hooks, and MCP configuration.
- **Tool lists do not reach children (runtimes 0.3.21 to 0.3.38; read from the runtime source, not tested).** A child whose agent definition sets no permission mode, including the built-in `General` agent, gets a fixed toolset (`read`, `glob`, `ls`, `grep`, `write`, `edit`, `bash`, `fetch`, `web_search`, `git_info`) whatever the session's `allowed_tools` and `denied_tools` say. Its tool calls raise no approval requests, even with `interactive_approvals` on. The filesystem policy and command sandbox still apply. Fixed in runtime 0.3.39: children get only the tools the session allows, and their calls ask when the session asks.
- **Delegation under policy:** `agent` is allowed at the secure baseline, so no explicit grant is needed. `denied_tools`, or an `allowed_tools` list that leaves it out, removes it. When tool restrictions or approvals matter, remove it.
- **Approvals:** with `interactive_approvals`, the `agent` call itself waits for an answer. Closing the session releases parked requests as denials.

### MCP servers

Sessions can start Model Context Protocol servers, with transactional startup (all-or-nothing) and teardown owned by the session:

```rust
use o4_agent_sdk::{ExecutionPolicy, McpServerConfig, SessionOptions};

let server = McpServerConfig::Stdio {
    name: "docs".into(),
    // An installed, trusted server; prefer an absolute path.
    command: "/usr/local/bin/docs-mcp-server".into(),
    args: vec![],
    env: Default::default(),
    cwd: None,
};
let options = SessionOptions::new("claude-sonnet-4-6", "/path/to/project")
    .execution_policy(ExecutionPolicy {
        // MCP tools are named `<server>__<tool>` and must be listed.
        allowed_tools: vec!["docs__search".into(), "read".into()],
        ..ExecutionPolicy::default()
    })
    .mcp_server(server);
```

The runtime names each MCP tool `<server name>__<tool name>`. Unless `sandbox_mode` is `Disabled`, the session keeps an MCP tool only when `allowed_tools` names it, and session creation fails when no MCP tool from any configured server is named. With several servers, one whose tools are all unnamed still starts, and its tools are dropped. A stdio server is started as an ordinary process outside the session's sandbox, before any model turn, so only configure servers you trust.

`McpServerConfig::Sse { name, url, headers }` connects to remote SSE servers, and `McpServerConfig::StreamableHttp` to Streamable HTTP servers. If any configured server fails to start, session creation fails and every successfully started server is shut down.

### Transport abstraction

The SDK talks to the runtime through a small async trait (shown in part; see the transports guide in the skill for all of it):

```text
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn send_line(&self, line: String) -> Result<(), TransportError>;
    /// Returns `None` at permanent EOF.
    async fn receive_line(&self) -> Result<Option<String>, TransportError>;
}
```

(The `Result` here is `std::result::Result` with `TransportError` -- transport implementations report raw I/O failures, not SDK-level errors.)

`LocalRuntime` is the built-in `TransportSource` that spawns `o4-agent-runtime` as a child process over piped stdin/stdout -- no PTY, with bounded stderr capture for diagnostics. Because the contract is plain byte-stream JSONL, a future remote/SSH adapter can bridge the same streams without changing any client code.

## The wire protocol

Commands and events are framed as one JSON object per line, versioned as protocol v1.

**Commands** (`Command`, tagged by `type`): `initialize`, `session_create`, `session_resume`, `run_start`, `run_cancel`, `run_steer`, `session_close`, `shutdown`; for hosted sessions and work, `session_list`, `session_attach`, `session_detach`, `work_list`, `work_action`, `approval_respond`; and the wire-only `question_answer`, `run_drain`, `session_ownership`, which have no `Client` method.

**Events** (`Event`, tagged by `type`):

- Sessions: `runtime_ready`, `session_created`, `session_resumed`, `session_closed`.
- Run progress: `run_started`, `agent_started`, `turn_started`, `message_started`, `message_updated`, `message_completed`, `tool_started`, `tool_updated`, `tool_completed`, `run_completed`. Message and tool events carry whole snapshots, not text deltas.
- Diagnostics: `warning`, `router_diagnostic`, `retry`, `context_compacted`, `resource_limit`, `steering_injected`.
- Replies: `command_ack`, `error`.
- Hosted sessions, work and approvals: `session_listed`, `session_attached`, `session_detached`, `work_listed`, `work_actioned`, `work_changed`, `approval_requested`; wire-only `question_asked`, `run_drained`, `session_ownership_reported`.
- Any other tag is kept as `Event::Unknown`, so a newer runtime does not break an older SDK.

Compatibility is enforced at connect time on three axes:

1. **Protocol version** -- must be mutually supported (v1 today).
2. **Capabilities** -- any capability you require must be advertised by the runtime.
3. **Runtime version** -- must satisfy the SDK's semver requirement.

## Compatibility

The SDK pins the exact runtime it was built and verified against in `compatibility.toml`:

```toml
runtime_commit = "ea92fb224ae10562e2b30e08c20ac0ac302d02e8"
runtime_version = "0.3.43"
protocol_version = 1
runtime_version_requirement = ">=0.0.57, <1.0.0"
```

The default requirement `>=0.0.57, <1.0.0` is compiled into the SDK and checked during the handshake: every 0.x runtime is accepted, because o4's minor number moves when its patch number passes 99, not on a protocol change. A runtime reporting `0.0.56` or `1.0.0` is rejected. Protocol version and capabilities are the real compatibility checks; `runtime_version` records the release the SDK was last verified against.

## Agent runtime

`o4-agent-runtime` is the server half, published with every o4 release (see [Installation](#installation)). It reads JSONL commands from stdin and writes JSONL events to stdout.

Highlights:

- Reuses the real `o4_agent::agent_loop` -- no reimplementation.
- Durable session definitions on disk, restorable across processes.
- Transactional, session-owned MCP startup and teardown.
- Fail-closed execution-policy enforcement for filesystem, process, and network access.
- Bounded frame and response sizes; redacted diagnostics.

Download it from the o4 release that matches your pin.

## Error handling

All fallible SDK calls return `o4_agent_sdk::Result<T>`. The `Error` enum is `#[non_exhaustive]`; the current variants are:

| Variant | Meaning |
|---|---|
| `InvalidRuntimePath` | runtime executable path failed validation |
| `InvalidWorkingDirectory` | session working directory failed validation |
| `Transport` | transport-level failure (spawn, I/O, framing) |
| `Serialization` | failed to encode a protocol request |
| `MalformedProtocol` | runtime emitted malformed protocol output |
| `ProtocolViolation` | runtime broke the wire protocol |
| `ProtocolVersionMismatch` | no mutually supported protocol version |
| `InvalidRuntimeVersion` | runtime reported a non-semver version |
| `RuntimeVersionIncompatible` | runtime version outside the required range |
| `MissingCapabilities` | a required capability is missing |
| `HandshakeTimeout` | runtime handshake timed out |
| `RuntimeExited` | runtime process ended unexpectedly (carries redacted `RuntimeDiagnostics`) |
| `Runtime` | the runtime rejected a command (`code`, `message`, `retryable`) |
| `ClientShutdown` | the client has been shut down |
| `SessionClosed` | the session is closed |
| `RunFinished` | the run is no longer active |
| `EventBufferOverflow` | the event consumer fell behind |

`RuntimeDiagnostics` (bounded captured stderr) is deliberately redacted from `Debug`/`Display` so routine logging cannot leak credentials; access it only through an explicitly secured path.

## Testing

The SDK's integration suite runs against an in-memory fake JSONL runtime, so it needs no provider API keys:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo test --workspace --doc      # compiles the Rust code in these docs
cargo clippy --workspace --all-targets -- -D warnings
```

The end-to-end tests exercise the real runtime binary (set `O4_AGENT_RUNTIME_BIN`). They cover the handshake and capabilities, session create and close, a session resumed by a second runtime process, and rejection of an invalid working directory.

`scripts/run-examples.sh /path/to/o4-agent-runtime` runs every example against a real runtime with its built-in offline test model, so it needs no provider account: one-shot and persistent sessions, a schema run, live progress, resuming after a restart, an MCP server, and approving and refusing a file write. CI runs it against the published runtime.

## Security notes

- The SDK never handles provider API keys; the runtime does, within its own process.
- Execution policy is enforced by the runtime, fail-closed -- the SDK only declares intent.
- A stdio MCP server runs as an ordinary process outside the session's sandbox; configure only servers you trust.
- Runtime stderr is captured into a bounded, redacted buffer.
- The JSONL transport carries no terminal control sequences, so it is safe to bridge over untrusted byte streams.
