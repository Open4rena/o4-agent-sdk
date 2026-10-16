# Permissions, external tools, and delegated agents

Applies to o4 Agent SDK v0.4.1. The SDK declares a policy; the **runtime** enforces it. Ask the user which workspace, commands, network destinations, and external services the application should be allowed to use. Do not choose broader access to make a failing example pass.

## Execution policy

Attach `ExecutionPolicy` when creating the session with `SessionOptions::execution_policy`. Its fields are:

| Field | Meaning |
|---|---|
| `allowed_tools` | Explicit grants beyond the runtime's secure baseline; **not** a replacement for all baseline tools |
| `denied_tools` | Tools to remove, including otherwise baseline tools |
| `sandbox_mode` | `ReadOnly` (default), `WorkspaceWrite`, or `Disabled` |
| `sandbox_config` | Optional runtime-specific JSON configuration; use a documented, verified schema for that runtime, not guessed keys |
| `allowed_domains` | Explicit network grants interpreted by runtime policy; empty is not unrestricted access |
| `interactive_approvals` | Opt in to permission requests; default false, requires runtime support and an active approval handler |

Empty allow lists mean **no explicit grants**, not "all tools/domains" and not "no tools whatsoever." Static policy and sandbox enforcement still apply. Unknown or ambiguous tool names are rejected instead of silently weakening policy. Do not claim an `allowed_tools` list alone is an exhaustive allowlist.

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

This is a starting policy, not proof of complete isolation. Review every assembled capability, including plugins, MCP tools, process execution, and delegation, against the application's threat model. In restricted profiles the runtime rejects capabilities it cannot enforce. Tool provenance is host-owned: an MCP/plugin tool cannot acquire a built-in's privileges by reusing its name.

For a coding job allowed to modify its project, discuss `WorkspaceWrite` and narrowly scoped grants with the user. Do not switch to `Disabled` or grant arbitrary shell/network access as a fallback. Verify allowed **and denied** behavior in a disposable workspace; a successfully serialized policy is not proof of runtime enforcement.

A session's saved definition fixes its policy. `ResumeOptions` and `AttachedSession` cannot widen it. Create a new session if a deliberately different policy is needed. Do not equate a human approval with bypassing an otherwise denied capability.

## Interactive approval is a separate workflow

Before setting `interactive_approvals: true`, read [Hosted sessions, work, and approvals](hosted-sessions-work-and-approvals.md). The application must obtain `client.approvals()` once, then continuously receive and answer requests alongside the run. Without a handler the run can wait indefinitely. With approvals off, keep static allow/deny behavior; do not promise a dialog in a headless job.

## Session-owned MCP servers

`SessionOptions::mcp_server(config)` appends a server; it can be called repeatedly. MCP startup is transactional: a configured startup failure fails session creation and tears down servers already started. Session teardown owns the connections. The model calls exposed MCP tools through the runtime; the SDK does not expose a separate `client.call_tool()` API.

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

Use an installed, verified executable and explicitly scoped server arguments (for example, the intended filesystem root). Do not silently install packages with an unpinned `npx -y` command. The fields are `name`, `command`, `args`, `env`, and optional `cwd`. `env` values are literal process environment values, not an SDK secret-store or interpolation syntax. Keep secrets out of arguments and out of saved definitions whenever possible; understand the runtime/server credential flow before using this field for credentials.

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
- Children inherit the parent's effective model/reasoning, credential resolution, filesystem policy, hooks, and MCP configuration. They must not gain broader access than the parent.
- The agent tool is part of the runtime's secure baseline; deny `agent` explicitly when delegation is not wanted.
- With interactive approvals enabled, child approval requests use the same application approval path. Without it, a child needing approval fails closed instead of waiting for a nonexistent operator.

Ask for delegation only when it helps the task and fits approved access/cost limits. Do not promise a specific decomposition, parallelism level, or SDK child handle merely because the prompt requests it. Work tracking, when advertised, can expose runtime registry rows for sub-agents; see [Hosted sessions and work](hosted-sessions-work-and-approvals.md).
