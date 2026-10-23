# Permissions, external tools, and delegated agents

Applies to o4 Agent SDK v0.4.1. The SDK declares a policy; the **runtime** enforces it. Ask the user which workspace, commands, network destinations, and external services the application should be allowed to use. Do not choose broader access to make a failing example pass.

## Execution policy

Attach `ExecutionPolicy` when creating the session with `SessionOptions::execution_policy`. Its fields are:

| Field | Meaning |
|---|---|
| `allowed_tools` | Empty: the sandbox mode's baseline tools. Non-empty: **only** the listed tools, which replaces the baseline (see below) |
| `denied_tools` | Tools to remove, including otherwise baseline tools; a tool both allowed and denied is an error |
| `sandbox_mode` | `ReadOnly` (default), `WorkspaceWrite`, or `Disabled` |
| `sandbox_config` | Optional runtime-specific JSON configuration; use a documented, verified schema for that runtime, not guessed keys |
| `allowed_domains` | Explicit network grants interpreted by runtime policy; empty is not unrestricted access |
| `interactive_approvals` | Opt in to permission requests; default false, requires runtime support and an active approval handler |

With `allowed_tools` empty, the session gets the sandbox mode's baseline: under `ReadOnly`, the read-only file tools, internal helpers, the `agent` delegation tool, and network tools only when `allowed_domains` is set; `WorkspaceWrite` adds the file-writing tools. With `allowed_tools` non-empty, the session gets exactly the listed tools and nothing from the baseline, so name every tool the job needs, including `read`, `ls`, `glob`, and `grep`. Verified on runtime 0.3.21: a `WorkspaceWrite` session allowing only `read` could not write.

Other rules in runtimes 0.3.21 to 0.3.34: an unknown or ambiguous tool name fails session creation instead of being ignored; a `ReadOnly` policy cannot grant a file-writing or process tool; process tools such as `bash` need an explicit grant and an enforcing OS sandbox; network tools need at least one `allowed_domains` entry. An empty `allowed_domains` grants no network access. Never treat a successful session creation as proof that a tool is present: verify behavior.

A conservative starting point for a user-approved read-only application is:

```rust
use o4_agent_sdk::{ExecutionPolicy, SandboxMode, SessionOptions};

fn read_only_options(model: &str, cwd: &std::path::Path) -> SessionOptions {
    SessionOptions::new(model, cwd).execution_policy(ExecutionPolicy {
        sandbox_mode: SandboxMode::ReadOnly,
        denied_tools: vec!["bash".into(), "write".into(), "edit".into()],
        ..ExecutionPolicy::default()
    })
}
```

This is a starting policy, not proof of complete isolation. `ReadOnly` already leaves out the denied tools; listing them protects the session if its mode is later loosened. The baseline still includes the `agent` delegation tool; deny `agent` too when delegation is not wanted. Review every assembled capability, including plugins, MCP tools, process execution, and delegation, against the application's threat model. In restricted profiles the runtime rejects capabilities it cannot enforce. Tool provenance is host-owned: an MCP/plugin tool cannot acquire a built-in's privileges by reusing its name.

For a coding job allowed to modify its project, discuss `WorkspaceWrite` and narrowly scoped grants with the user. Do not switch to `Disabled` or grant arbitrary shell/network access as a fallback. Verify allowed **and denied** behavior in a disposable workspace; a successfully serialized policy is not proof of runtime enforcement.

A session's saved definition fixes its policy. `ResumeOptions` and `AttachedSession` cannot widen it. Create a new session if a deliberately different policy is needed. Do not equate a human approval with bypassing an otherwise denied capability.

## Interactive approval is a separate workflow

Before setting `interactive_approvals: true`, read [Hosted sessions, work, and approvals](hosted-sessions-work-and-approvals.md). The application must obtain `client.approvals()` once, then continuously receive and answer requests alongside the run. Without a handler the run can wait indefinitely. With approvals off, keep static allow/deny behavior; do not promise a dialog in a headless job.

## Session-owned MCP servers

`SessionOptions::mcp_server(config)` appends a server; it can be called repeatedly. MCP startup is transactional: a configured startup failure fails session creation and tears down servers already started. Session teardown owns the connections. The model calls exposed MCP tools through the runtime; the SDK does not expose a separate `client.call_tool()` API.

**Grant MCP tools by name.** In runtimes 0.3.21 to 0.3.34 the runtime names each MCP tool `<server name>__<tool name>`, such as `workspace-tools__search` for a server named `workspace-tools`. Unless `sandbox_mode` is `Disabled`, a session keeps an MCP tool only if `allowed_tools` names it that way, and session creation fails when no MCP tool from any configured server is named. With several servers, one whose tools are all unnamed still starts, and its tools are dropped without a warning. With `allowed_tools` empty, the runtime refuses the session before starting any server. Because a non-empty list replaces the baseline, also name the built-in tools the job needs; for example, `vec!["workspace-tools__search".into(), "read".into(), "ls".into(), "glob".into(), "grep".into()]`. Granting MCP tools is not a reason to choose `Disabled`. The [mcp_tools example](https://github.com/Open4rena/o4-agent-sdk/blob/main/crates/o4-agent-sdk/examples/mcp_tools.rs) runs this against a small local server.

**A stdio server is not sandboxed.** The runtime starts it as an ordinary process that runs outside the session's sandbox, before any model turn, even in a `ReadOnly` session: verified on runtime 0.3.21, where a server started by a read-only session wrote a file in the workspace. Its permissions are those of the runtime process. Treat configuring a server as running that program with the application's privileges, and only configure servers you trust. The runtime gives a stdio server a reduced environment (basic variables such as `PATH` and `HOME`, plus its `env` entries), not the runtime's provider keys. Interactive approval applies to the model's MCP tool calls, not to the server process itself.

Require the runtime's `mcp` capability when MCP is essential. The protocol supports three variants below, but a generic capability is not a transport-specific version guarantee: verify the chosen runtime with the actual server. The SDK has no separate minimum-runtime or capability constant specifically for Streamable HTTP.

The constructors below take caller-approved executables, arguments, destinations, and nonsecret headers. They do not download or execute anything by themselves. A session using them may start the process or contact the server, so get approval before doing that. All examples import the types through `o4_agent_sdk`; a separate protocol-crate dependency is not needed.

### Local stdio

```rust
use std::collections::BTreeMap;
use o4_agent_sdk::McpServerConfig;

fn stdio_server(
    executable: String,
    args: Vec<String>,
    cwd: Option<String>,
) -> McpServerConfig {
    McpServerConfig::Stdio {
        name: "workspace-tools".into(),
        command: executable,
        args,
        env: BTreeMap::new(),
        cwd,
    }
}
```

Use an installed, verified executable and explicitly scoped server arguments (for example, the intended filesystem root). Do not silently install packages with an unpinned `npx -y` command. The fields are `name`, `command`, `args`, `env`, and optional `cwd`. `command` is looked up on the `PATH` the runtime passes to the server, so prefer an absolute path. `env` values are literal process environment values, not an SDK secret-store or interpolation syntax. Keep secrets out of arguments and out of saved definitions whenever possible; understand the runtime/server credential flow before using this field for credentials.

### SSE

```rust
use std::collections::BTreeMap;
use o4_agent_sdk::McpServerConfig;

fn sse_server(url: String, nonsecret_headers: BTreeMap<String, String>) -> McpServerConfig {
    McpServerConfig::Sse {
        name: "remote-tools".into(),
        url,
        headers: nonsecret_headers,
    }
}
```

`Sse` has literal `url` and `headers` fields. Do not invent `bearer_token_env_var` for this variant or assume shell-style environment expansion. Avoid credentials in URLs. Inline header secrets are serialized to the runtime and may be saved in durable session configuration; choose a reviewed credential approach rather than treating redacted debug output as encryption.

### Streamable HTTP

```rust
use std::collections::BTreeMap;
use o4_agent_sdk::McpServerConfig;

fn http_server(url: String) -> McpServerConfig {
    McpServerConfig::StreamableHttp {
        name: "http-tools".into(),
        url,
        headers: BTreeMap::new(),
        bearer_token_env_var: Some("MCP_ACCESS_TOKEN".into()),
        env_http_headers: BTreeMap::from([
            ("X-Workspace".into(), "MCP_WORKSPACE_ID".into()),
        ]),
    }
}
```

`bearer_token_env_var` names the variable the **runtime** reads for the bearer token. `env_http_headers` maps **header name to environment-variable name**, not the other way round. Put actual values in the child runtime's launch environment before connection; do not send token values as variable names or embed them in prompts. Nonsecret static headers can use `headers`. Confirm the destination and use authenticated, encrypted network connections where appropriate to the deployment.

`McpServerConfig`'s custom `Debug` redacts sensitive fields, but serialization contains configuration needed by the runtime. Do not log raw serialized configurations, full tool arguments/results, or secret-bearing URLs. Use per-server identifiers and sanitized status for routine diagnostics.

## Delegated agents

There is **no** public Rust `Session::spawn_agent` or `Client::agent` method. When the runtime exposes and permits its agent tool, the model can delegate based on the prompt. This is different from starting independent SDK sessions yourself.

For the recorded runtime behavior:

- The root is depth 0; children can exist at depths 1–3. A depth-3 agent cannot spawn another. This is a fixed runtime limit, not an SDK `max_depth` option.
- Children inherit the parent's effective model/reasoning, credential resolution, filesystem policy, hooks, and MCP configuration.
- **Tool lists and approvals do not reach most children (runtimes 0.3.21 to 0.3.35; read from the runtime source, not tested; a fix is due in a future runtime release, none published yet).** A child whose agent definition sets no permission mode, including the built-in `General` agent, gets a fixed toolset (`read`, `glob`, `ls`, `grep`, `write`, `edit`, `bash`, `fetch`, `web_search`, `git_info`), whatever the session's `allowed_tools` and `denied_tools` say. Its tool calls raise no approval requests, even with `interactive_approvals` on; only the `agent` call itself waits for an answer. The session's filesystem policy and command sandbox still apply. When tool restrictions or approvals matter, leave `agent` out of the session.
- The agent tool is part of the runtime's secure baseline; deny `agent` explicitly when delegation is not wanted. A non-empty `allowed_tools` list that does not name `agent` also leaves it out.

Ask for delegation only when it helps the task and fits approved access/cost limits. Do not promise a specific decomposition, parallelism level, or SDK child handle merely because the prompt requests it. Work tracking, when advertised, can expose runtime registry rows for sub-agents; see [Hosted sessions and work](hosted-sessions-work-and-approvals.md).
