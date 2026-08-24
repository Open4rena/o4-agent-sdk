# o4 Agent SDK for Rust

A small, standalone async SDK that controls the real `o4-agent-runtime` process over a versioned JSONL protocol. The SDK is a control plane: **it does not implement an agent loop**, call model providers, or depend on sshable.

This repository is an MVP workspace:

- [`o4-agent-protocol`](crates/o4-agent-protocol) contains serde wire types and v1 compatibility constants.
- [`o4-agent-sdk`](crates/o4-agent-sdk) contains the public async API, local subprocess transport, abstract transport contract, and event router.

## Architecture

```text
application
    |
Client / Session / RunHandle
    |
request IDs + one stdout reader/router
    |
Transport (ordinary framed byte streams; no terminal API)
    |
LocalRuntime: piped stdin/stdout/stderr
    |
o4-agent-runtime (owns the agent loop and durable state)
```

Every input is one JSON object followed by `\n`. Every output is one typed event envelope followed by `\n`. Concurrent requests and runs are correlated by `request_id` and checked against `session_id`/`run_id`. A single reader task validates the global monotonic sequence, RFC3339 timestamps, runtime identity, negotiated protocol, and event shape before routing output. Malformed output, an unknown request ID, a sequence regression, or unexpected EOF fails all pending handles.

`Transport` exposes only `send_line`, `receive_line`, graceful `close`, best-effort `abort`, and bounded diagnostics. It has no process, shell, terminal, or PTY assumptions. A future remote adapter—including one supplied by sshable—can implement this trait over ordinary reliable byte streams. This crate intentionally has **no sshable dependency**.

## Wire compatibility

The current contract is:

- protocol version: **v1** (`PROTOCOL_VERSION`)
- SDK-supported protocols: **`[1]`** (`SUPPORTED_PROTOCOL_VERSIONS`)
- default runtime semver requirement: **`>=0.0.0`** (`DEFAULT_RUNTIME_VERSION_REQUIREMENT`)

The default semver floor is deliberately permissive so the SDK can connect to any protocol-v1 runtime; the authoritative runtime pin lives in [`compatibility.toml`](compatibility.toml), which records the exact o4 runtime commit and version this SDK was built and verified against. Applications that need a hard guarantee can pin a stricter range themselves:

```rust,no_run
use o4_agent_sdk::ClientOptions;
use semver::VersionReq;

let options = ClientOptions::default()
    .runtime_version_requirement(VersionReq::parse(">=1.4.0, <2.0.0")?);
# Ok::<(), semver::Error>(())
```

Compatibility is negotiated in this order:

1. `initialize` advertises the client's supported protocol versions and capabilities.
2. `runtime_ready` advertises the runtime's protocol versions and capabilities.
3. The SDK requires a common protocol version and any explicitly required capabilities.
4. Runtime semver is parsed and checked independently against `ClientOptions`; semver never substitutes for protocol negotiation.

Protocol structs intentionally do not apply blanket `deny_unknown_fields`, so additive v1 fields do not break older clients. Known events are strongly typed. Unknown event tags deserialize to `Event::Unknown`, preserving their fields; applications should inspect `RuntimeInfo.capabilities` before relying on optional event behavior. Unknown command tags remain errors because the SDK only sends commands it understands.

## Security defaults

- `LocalRuntime::new(path)` canonicalizes the runtime path, validates that it is a file, and checks that it is executable on Unix. It does not search `PATH`.
- Session creation and resume validate `cwd` before sending it.
- The runtime is launched directly, without a shell or PTY, with piped stdin/stdout/stderr.
- Only stdout is parsed as protocol JSONL. Human output is never guessed or parsed; any non-JSON stdout line is a fatal protocol error.
- Stderr is drained separately into a bounded tail buffer (64 KiB by default) to prevent child-process blockage and unbounded memory growth.
- `RuntimeDiagnostics` redacts contents in `Debug`/`Display`; explicitly calling `as_str()` is a sensitive diagnostic operation.
- MCP stdio environment values and SSE URL/header values are serialized for the runtime but redacted from `Debug`; stdio arguments are also redacted because they may contain tokens.
- `ExecutionPolicy::default()` is read-only with no explicit tool or domain grants. The runtime remains the final policy enforcement boundary.
- `Session::close()` unloads only. Dropping `Session` sends nothing and never deletes durable data.
- `Client::shutdown()` is explicit and graceful. Dropping the final client without it invokes the transport's kill/abort fallback.
- Public errors avoid MCP and policy payloads. Transport implementations should likewise keep secrets out of their error strings.

## Quick start

Add the SDK crate and Tokio, then connect to the actual runtime executable:

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::connect(LocalRuntime::new("/path/to/o4-agent-runtime")).await?;
    let result = client
        .run(
            "Summarize this workspace.",
            SessionOptions::new("o4", std::env::current_dir()?),
        )
        .await?;
    println!("{}", result.output);
    client.shutdown().await?;
    Ok(())
}
```

The full streaming example uses `RunHandle::next_event`:

```sh
cargo run -p o4-agent-sdk --example one_shot -- \
  /path/to/o4-agent-runtime "Summarize this workspace"
```

See [`one_shot.rs`](crates/o4-agent-sdk/examples/one_shot.rs).

## Persistent and resumed sessions

```rust,no_run
use o4_agent_sdk::{Client, LocalRuntime, ResumeOptions, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::connect(LocalRuntime::new("/path/to/o4-agent-runtime")).await?;
    let cwd = std::env::current_dir()?;
    let session = client
        .create_session(
            SessionOptions::new("o4", &cwd).session_id("durable-project-session"),
        )
        .await?;

    let first = session.run("Remember: codename Kestrel").await?
        .final_result().await?;
    println!("{}", first.output);
    session.close().await?; // unload only

    let resumed = client
        .resume_session("durable-project-session", ResumeOptions::default().cwd(&cwd))
        .await?;
    let second = resumed.run("What is the codename?").await?
        .final_result().await?;
    println!("{}", second.output);

    resumed.close().await?;
    client.shutdown().await?;
    Ok(())
}
```

Run the complete example with:

```sh
cargo run -p o4-agent-sdk --example persistent -- /path/to/o4-agent-runtime
```

## Streaming, cancellation, and steering

`Session::run` returns immediately after sending `run_start`; `RunHandle` is both a `futures_core::Stream<Item = Result<EventEnvelope>>` and a `next_event()` source. `final_result()` consumes it and waits for `run_completed`. Once `run_started` supplies the ID, `cancel()` and `steer()` issue independently correlated commands while the run remains active.

```rust,no_run
# use o4_agent_sdk::{Client, LocalRuntime, SessionOptions};
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let client = Client::connect(LocalRuntime::new("/path/to/o4-agent-runtime")).await?;
# let session = client.create_session(SessionOptions::new("o4", std::env::current_dir()?)).await?;
let run = session.run("Perform a long review").await?;
run.steer("Prioritize correctness over style").await?;
run.cancel().await?;
let result = run.final_result().await?;
println!("status: {:?}", result.status);
# session.close().await?;
# client.shutdown().await?;
# Ok(())
# }
```

## Testing and development

The integration suite uses an in-memory fake JSONL runtime. It needs no provider keys and covers serialization, protocol mismatch, separate semver/capability checks, concurrent event routing, one-shot and persistent/resumed flows, cancel/steer, sequence rejection, malformed output, process EOF diagnostics, MCP/policy wire fields and redaction, path validation, and transport operation without a PTY.

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```
