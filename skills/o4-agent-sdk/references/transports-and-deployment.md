# Runtime setup, deployment, custom transports, and troubleshooting

Applies to o4 Agent SDK v0.4.1. This is a Rust async library controlling a separate process, not a provider client or a bundled runtime.

## Install the two artifacts

Use Rust 1.88+ and a published SDK tag. For a new application:

```toml
[dependencies]
o4-agent-sdk = { git = "https://github.com/Open4rena/o4-agent-sdk", tag = "v0.4.1" }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time"] }
```

Add `serde_json = "1"` for JSON schemas/values you construct yourself, `semver = "1"` for a custom version requirement, or `async-trait = "0.1"` for a transport implementation only when needed. Do not assume transitive dependencies can be imported directly by the application.

Get the standalone runtime from [public o4 releases](https://github.com/Open4rena/o4-releases), **not** the private o4 repository. The recorded verified release is `0.3.21`. Choose the `o4-agent-runtime-<platform>.tar.gz` asset and matching `.sha256` for `linux-x86_64`, `linux-arm64`, or `macos-arm64`; do not mistake a CLI archive for the runtime. Verify its checksum before extracting/running it (`sha256sum -c` on Linux or `shasum -a 256 -c` on macOS). The archive holds `o4-agent-runtime`; it needs no `o4` CLI and takes no arguments. Do not invent an `o4 agent-runtime` subcommand or use a terminal/PTY.

`LocalRuntime::new(path)` requires an existing executable file path. It canonicalizes the path and does **not** search `PATH`; prefer an absolute path or an explicit relative path such as `./o4-agent-runtime`. `O4_AGENT_RUNTIME_BIN` is a test/application convention: the SDK does not automatically read it for you.

`SessionOptions::new(model, cwd)` needs a model the runtime has registered, such as `claude-sonnet-4-6`; there is no default model, and an unregistered name such as `o4` fails session creation with `Error::Runtime` (`model '<name>' is not registered`). A `provider:model` form such as `anthropic:claude-sonnet-4-6` selects the provider explicitly.

Runtimes 0.3.21 to 0.3.34 include an offline test model, `open4rena-smoke:open4rena-usage-smoke`. It needs no account, makes no network request and costs nothing. It returns fixed replies, never real answers: `O4_PROVIDER_KEY_SMOKE_OK` to an ordinary prompt; for the prompt `O4_EVAL_HARNESS_WRITE_SMOKE`, a call to the `write` tool for `eval-harness-smoke.txt` in the workspace, then `O4_EVAL_HARNESS_WRITE_SMOKE_OK` or `..._FAILED`. It cannot produce structured output. Use it to test wiring, permissions and approvals without a provider; [`scripts/run-examples.sh`](https://github.com/Open4rena/o4-agent-sdk/blob/main/scripts/run-examples.sh) runs every SDK example with it. It is not a supported product model and may change between runtime releases.

## Connect, inspect support, and clean up

This helper requires the caller to provide the verified runtime path, configured model, and existing workspace. It attempts graceful shutdown even when the task fails. The caller then checks the returned `RunStatus`, not just `Ok`.

```rust,no_run
use std::path::Path;
use std::time::Duration;
use o4_agent_sdk::{Client, ClientOptions, LocalRuntime, RunResult, SessionOptions};

async fn owned_job(
    executable: &Path,
    data_dir: &Path,
    model: &str,
    cwd: &Path,
) -> o4_agent_sdk::Result<RunResult> {
    let runtime = LocalRuntime::new(executable)
        .data_dir(data_dir)
        .stderr_limit(64 * 1024);
    let options = ClientOptions::default()
        .client_identity("my-application", "1.0.0")
        .handshake_timeout(Duration::from_secs(10));
    let client = Client::connect_with_options(runtime, options).await?;
    let info = client.runtime_info();
    println!("runtime {} / protocol {}", info.runtime_version, info.protocol_version);
    let outcome = client
        .run("Summarize this workspace", SessionOptions::new(model, cwd))
        .await;
    let shutdown = client.shutdown().await;
    let result = outcome?;
    shutdown?;
    Ok(result)
}
```

`Client::connect(source)` uses default options. `LocalRuntime::path()` reports its configured path. `ClientOptions` builders are `client_identity`, `handshake_timeout`, `require_capability`, `advertise_capability`, and `runtime_version_requirement` (taking `semver::VersionReq`). The default handshake timeout is ten seconds; increase it only for a justified environment, not to hide malformed protocol or missing support.

Compatibility has **three independent checks**:

1. Protocol must be mutually supported (`PROTOCOL_VERSION == 1`; `SUPPORTED_PROTOCOL_VERSIONS` lists SDK support).
2. Required capabilities must be advertised by the runtime.
3. The runtime must satisfy the SDK's default `DEFAULT_RUNTIME_VERSION_REQUIREMENT`, `>=0.0.57, <1.0.0`, or an explicitly chosen stricter requirement. Acceptance in that range does not certify every optional feature/provider.

`SDK_CAPABILITIES` lists the client's default advertisements. `ClientOptions::advertise_capability` describes behavior the application actually supports; it does not cause the runtime to implement a feature. `require_capability` enforces the runtime side. `RuntimeInfo` exposes `runtime_version`, `protocol_version`, and `capabilities` after a successful connection. Do not confuse the protocol crate's placeholder version requirement with the SDK's enforced default.

For example, if a schema result is essential, require it at connection:

```rust
use o4_agent_sdk::ClientOptions;

fn structured_client_options() -> ClientOptions {
    ClientOptions::default().require_capability("structured_output")
}
```

Other feature names include `mcp`, `session.resume`, `run.cancel`, `run.steer`, and the gated hosted-session/work/approval capabilities. Only require features the application actually needs, and handle absence honestly. Do not widen semver or skip negotiation to get past an incompatibility. Changing a tested pin is a user-approved, verified action; SDK and runtime versions do not advance together.

`Client::shutdown()` is the graceful runtime-wide shutdown path. Its effect is shared by all client clones. Dropping every SDK handle uses best-effort transport abort instead, not graceful completion. Session and run handles can keep the connection alive. For a custom connection to a runtime other people share, disconnect/detach through the adapter's lifecycle; do not send shutdown just because one UI closes.

## Provider setup and state on servers

`LocalRuntime` inherits the application's process environment. It has no `.api_key(...)`, per-session `.env(...)`, or arbitrary launch-argument builder. Configure credentials and launch environment through the application's deployment before spawning; avoid changing global environment variables from concurrent Rust tasks. Use an appropriate separately launched/custom transport if the application needs distinct child environments.

| Data | Runtime lookup |
|---|---|
| Durable sessions, session database, `config.toml` | Runtime data directory (default `$HOME/.o4`) |
| `providers.toml`, `catalog/`, `models.toml`, saved provider keys in `settings.json` | `$HOME/.o4`, **not** moved by `O4_AGENT_RUNTIME_DATA_DIR` |

`LocalRuntime::data_dir(path)` sets `O4_AGENT_RUNTIME_DATA_DIR` for its child. Persist that directory across restarts for cold resume. It does not move provider configuration or isolate credentials by itself. Keep data files private and give the runtime a stable workspace it can actually access.

Built-in providers can read standard variables such as `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`; no prepopulated home is necessary for that setup. Use a model registered by the chosen provider/runtime. For a custom OpenAI-compatible provider, place the runtime-supported `providers.toml` in `$HOME/.o4` and refer to a secret variable with its `api_key_env` field. Follow the chosen runtime's documented manifest format instead of guessing fields. A manifest placed only in the session data directory will not register that model. World-writable manifests are skipped; set suitable ownership and non-world-writable permissions.

For containers/CI:

- Supply a writable, owned `HOME` for provider lookup and a persistent runtime data mount if resume is needed.
- Pass API-key credentials through the deployment's secret facility. Interactive browser sign-in is not a headless authentication setup; do not ask a server to open a browser.
- Run as an appropriately restricted user; choose the correct architecture. Static Linux runtime downloads do not mean external tool processes are bundled too.
- Pin/install required external executables explicitly. Validate MCP/tool availability and policy in that container, not only on a developer's machine.
- Keep stdout exclusively JSONL for the runtime. Send operational logs to stderr and protect them. Never include keys in prompts, copied configs, build output, or ordinary logs.
- Session metadata (`Context`) is not authentication, tenant isolation, or a substitute for separate process/filesystem/credential boundaries. Agree on that application design with the user.

## Custom byte-stream transports

The SDK has no built-in SSH, TCP, WebSocket, daemon-socket, or sshable connector. It supports adapters via these public traits:

- `Transport: Send + Sync + 'static`: async `send_line(String)`, async `receive_line() -> Result<Option<String>, TransportError>`, async `close()`, synchronous best-effort `abort()`, and `diagnostics() -> RuntimeDiagnostics`.
- `TransportSource: Send + 'static`: async `open(self) -> o4_agent_sdk::Result<Arc<dyn Transport>>`.

A type implementing `Transport` can be passed directly to `Client::connect`; it gets a blanket `TransportSource` implementation. `Arc<dyn Transport>` is also accepted. Implement a separate source/factory when establishing a connection requires work. Use `async-trait = "0.1"` for async trait implementations, following the SDK's existing test transport pattern.

The contract:

1. The SDK supplies serialized JSON **without** the trailing newline. Each send must produce exactly one complete framed line and serialize concurrent senders so their bytes never interleave.
2. Each receive yields one complete JSON line without the line terminator, `None` at permanent EOF, or a sanitized `TransportError`. The client owns exactly **one** reader/router. Do not add a competing reader.
3. An adapter enforces appropriate frame/resource bounds, flushes writes, and keeps logging/SSH banners/shell output off the protocol stream. No terminal or PTY is involved.
4. `close` releases owned resources after an acknowledged shutdown. Implement `abort` for abandoned clients and bounded `diagnostics` if the adapter owns resources; the default no-op methods are not sufficient cleanup for an owned process/network connection.
5. Provide authentication, encryption, authorization, and connection ownership appropriate to the application. JSONL framing is **not** a network security boundary. Do not expose unauthenticated remote execution or choose a transport-security design without user approval.

`TransportError::new(message)` and `.message()` carry displayable failure text: never put credentials or raw protocol payloads in it. `RuntimeDiagnostics::new`, `is_empty`, and `len` support bounded diagnostic handling. `Debug`/`Display` do not expose its contents; `.as_str()` deliberately does, so use it only in an explicitly secured diagnostic path. Redacted display does not scrub the stored bytes.

The `protocol` re-export supplies `RequestEnvelope`, `Command`, `EventEnvelope`, `Event`, and wire payloads for custom peers/tests. Unknown event tags retain their fields as `Event::Unknown`; commands must be ones the peer understands. Do not mutate shared wire definitions, use a raw transport to bypass SDK negotiation/routing, or present wire-only operations as supported `Client` methods.

## Error handling and debugging

All fallible SDK calls use `o4_agent_sdk::Result<T>`; `Error` is non-exhaustive, so external matches need a catch-all arm. Distinguish these cases:

| Failure | What to check |
|---|---|
| `InvalidRuntimePath` | Real executable file path, architecture, execute permission; do not assume PATH lookup |
| `InvalidWorkingDirectory` | Existing accessible directory; on resume verify the original directory, not a relocation |
| `Transport`, `RuntimeExited` | Process/connection lifecycle; inspect bounded stderr only through a secured path |
| `Serialization`, `MalformedProtocol`, `ProtocolViolation` | JSONL framing, unexpected stdout, envelope metadata, peer/protocol compatibility; do not ignore bad lines |
| `ProtocolVersionMismatch`, `InvalidRuntimeVersion`, `RuntimeVersionIncompatible` | Correct peer/version and tested compatibility, without weakening checks |
| `MissingCapabilities`, `MissingCapability` | Advertised features required at connect or by a gated method |
| `HandshakeTimeout` | Child startup and valid runtime-ready response; no browser/TTY prompt on this connection |
| `Runtime { code, message, retryable }` | Runtime rejection and its sanitized code; `retryable` is not permission to rerun side effects automatically |
| `ClientShutdown`, `SessionClosed`, `RunFinished` | Handle lifetimes or completion/control races. In SDK v0.4.1, `cancel()` and `steer()` also return `RunFinished` when the connection has failed; call `next_event()` or `final_result()` to get the real error. The default branch, not yet tagged, returns the connection error from them instead |
| `EventBufferOverflow` | Active-run consumer is too slow; drain it promptly and handle the failed shared connection |

For an apparent hang, first check whether the app is draining events/approvals, waiting on a selected but idle observer, or holding handles alive. Do not retry indefinitely, kill unrelated processes, or disable enforcement to make progress. On ambiguous connection failure a command may have run already; decide whether retry/resume is safe before submitting it again.

## Verification without overclaiming

- Unit/integration tests can implement an in-memory `Transport`; no runtime or provider key is needed. Verify emitted requests and route correct event envelopes, IDs, monotonic sequences, and timestamps rather than returning arbitrary JSON.
- Test expected success **and** denial/failure: final non-completed status, missing capabilities, cancellation races, malformed output, cleanup, slow consumers, and approval handling as relevant.
- Compile SDK examples and the application's chosen integration with its pinned dependencies. `cargo fmt --all -- --check`, `cargo test --workspace --all-targets`, `cargo test --workspace --doc` (which compiles this skill's Rust code), and `cargo clippy --workspace --all-targets -- -D warnings` are the SDK repository gates.
- A real-runtime check uses an explicit executable, isolated workspace/state, successful handshake, and clean shutdown. Set `O4_AGENT_RUNTIME_BIN` when running the SDK's real-runtime tests and inspect output for skips; some helpers can skip on connection failure too.
- The offline test model checks real sessions, tools, permissions and approvals end to end without a provider; the SDK's own examples run that way in its CI. It never checks answer quality, structured output, or a real provider request.
- The existing real-runtime suite covers negotiation, session create/close, cold resume, invalid working directories, and cancelling a run that waits for approval. It does not by itself prove every MCP transport, approval UI, actual provider request, or policy restriction. Test those separately in the intended environment, with authorization for external connections and provider spending.
