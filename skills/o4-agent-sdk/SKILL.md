---
name: o4-agent-sdk
description: Build Rust apps on the o4 Agent SDK and o4-agent-runtime - sessions, JSON results, streaming, cancellation, permissions, MCP tools, human approvals, work dashboards, custom transports.
---

# o4 Agent SDK: build and operate Rust applications

Use this skill when a user wants to embed or control o4 from Rust. It teaches the coding agent how to use the SDK; installing it does not add a tool or skill to an SDK runtime session.

The SDK is a **control plane**. Your application uses `Client`, `Session`, and `RunHandle` to drive a separate `o4-agent-runtime` process. The runtime owns model calls, tools, the agent loop, and durable state. Do not reimplement that loop or scrape the interactive CLI.

## User's task

$ARGUMENTS

Use the supplied task and conversation context. If no task was supplied, ask what the user wants to build rather than making unrelated changes.

## Start with the user's application

1. Inspect its Rust dependencies, existing integration, and tests before editing. Identify the SDK version actually in use; these guides describe SDK **v0.4.1**. Do not upgrade or change the runtime pin without approval.
2. Establish the task: a single job, a continuing conversation, a user-facing stream, external tools, a monitoring UI, or a custom connection. Ask only for missing decisions that affect implementation.
3. Establish the runtime executable's real path, model/provider setup, workspace, and state directory. `LocalRuntime` does **not** search `PATH` for a bare executable name.
4. Agree on file/process/network permissions and whether a human must approve actions. Do not silently widen policy, disable the sandbox, enable unattended approval, or add unrequested providers/MCP servers.
5. Read the relevant bundled guides below, implement the smallest supported path, and verify it in the user's application.

## Load the right guide

Resolve these links relative to the installed directory containing this `SKILL.md`, **not** the application's current directory. Loading the skill does not automatically read its references or change directories. If the host did not provide the skill's source path, locate the installed `o4-agent-sdk/SKILL.md` using its skill/plugin listing or file search. Do not assume the original SDK checkout or its `docs/` directory exists.

| User's task | Read |
|---|---|
| Install/verify the runtime, configure providers, deploy to CI/containers, negotiate compatibility | [Runtime, deployment, transports, and troubleshooting](references/transports-and-deployment.md) |
| One-shot jobs, multi-turn conversations, cold resume, context, structured JSON, limits, usage | [Sessions and runs](references/sessions-and-runs.md) |
| Live message/tool output, cancellation, steering, concurrent jobs | [Sessions and runs](references/sessions-and-runs.md) |
| Filesystem/process/network controls, MCP stdio/SSE/Streamable HTTP, delegated agents | [Permissions, MCP, and sub-agents](references/policy-mcp-and-subagents.md) |
| List/observe hosted sessions, work dashboards/actions, answer human approval requests | [Hosted sessions, work, and approvals](references/hosted-sessions-work-and-approvals.md) |
| Implement a transport, handle errors, shut down cleanly, test without provider credentials | [Runtime, deployment, transports, and troubleshooting](references/transports-and-deployment.md) |

For a new integration, read runtime setup and sessions/runs first. Read the permissions guide before enabling tools with side effects. Read the approval guide before setting `interactive_approvals`.

## Start from a runnable example

The SDK repository has a complete program for each common application. The SDK's CI runs each one against a published runtime with an offline test model; that checks the wiring, not a real model's behavior, and it does not reach a successful schema result, an MCP tool call, or a cancel during a long run. Adapt the closest one instead of assembling an application from fragments:

| Application | Example |
|---|---|
| Stream a run's progress live; cancel it on request | [`live_progress.rs`](https://github.com/Open4rena/o4-agent-sdk/blob/main/crates/o4-agent-sdk/examples/live_progress.rs) |
| JSON report checked against a schema, with turn/token/time limits | [`structured_report.rs`](https://github.com/Open4rena/o4-agent-sdk/blob/main/crates/o4-agent-sdk/examples/structured_report.rs) |
| Continue a conversation after the application restarts | [`resume_after_restart.rs`](https://github.com/Open4rena/o4-agent-sdk/blob/main/crates/o4-agent-sdk/examples/resume_after_restart.rs) |
| Give a session tools from an MCP server, with a minimal tool list | [`mcp_tools.rs`](https://github.com/Open4rena/o4-agent-sdk/blob/main/crates/o4-agent-sdk/examples/mcp_tools.rs) |
| A person approves each file change before it happens | [`human_approval.rs`](https://github.com/Open4rena/o4-agent-sdk/blob/main/crates/o4-agent-sdk/examples/human_approval.rs) |

Each runs with `cargo run -p o4-agent-sdk --example <name> -- <path-to-o4-agent-runtime> <model> ...` from an SDK checkout. With the offline test model `open4rena-smoke:open4rena-usage-smoke`, they run without a provider account (see the runtime guide). The examples on `main` can be newer than the SDK version an application uses; check the API against its pinned version.

## Rules that prevent broken integrations

- **Use a published SDK tag and runtime download.** The Rust dependency and runtime binary are separate artifacts. Public users do not need access to o4's private source repository.
- **Keep all three compatibility checks.** Protocol version, runtime semver, and advertised capabilities are separate checks. The default accepted runtime range is `>=0.0.57, <1.0.0`; `0.3.21` is the recorded verified runtime, not a promise that every optional feature works on every accepted version.
- **Require what the application needs.** Check `runtime_info().capabilities` or use `ClientOptions::require_capability`. Do not hide a missing feature by weakening requirements. Capability-gated methods also reject unsupported calls themselves.
- **A tool list replaces the defaults.** A non-empty `allowed_tools` gives the session only the listed tools; MCP tools must be listed as `<server>__<tool>`. A stdio MCP server runs outside the sandbox, so configure only trusted servers. Sub-agents whose definition sets no permission mode, including the built-in `General` agent, ignore the session's tool lists and approvals, so leave out `agent` when those matter. See the permissions guide.
- **Treat context as metadata, not authorization.** `workspace_id`, `user_id`, and `correlation_id` label events; they do not enforce tenant isolation. Separate credentials, workspaces, state, and runtime lifetimes where the application's security design requires it.
- **Check the task outcome.** `Ok(RunResult)` is not necessarily a completed task. Inspect `RunStatus` and require the structured value when the application needs JSON.
- **Keep reading.** Drain active-run events or call `final_result()` promptly. Message/tool updates are complete JSON snapshots, not token deltas. A slow active-run consumer can fail the shared connection; observer and work-notice streams may drop updates.
- **Cancellation is explicit.** Dropping a `RunHandle` does not cancel its run. Request cancellation and consume the terminal outcome before reporting that work stopped.
- **Resume is not relocation.** `ResumeOptions::cwd` asserts the original directory. Closing a session unloads it; it does not delete its saved data.
- **Approval streams must be serviced.** Take the single approval stream before creating an approval-enabled session and keep answering requests while runs proceed. An unanswered request can park work indefinitely. Never auto-approve because the model asked.
- **Own cleanup.** Close sessions when finished and call `Client::shutdown()` for a runtime your application owns, including error paths. Shutdown affects every client clone and hosted session on that runtime. An observer of a shared runtime must not shut it down merely to disconnect.
- **Protect sensitive data.** Provider credentials belong to the runtime environment or its configured store, not prompts or source code. MCP arguments/headers, messages, tool payloads, and stderr may contain secrets. Do not log them indiscriminately or expose `RuntimeDiagnostics::as_str()` in ordinary logs.

## Public API coverage map

These guides cover the public high-level SDK surface, not just its quick start:

| Surface | Guide |
|---|---|
| `LocalRuntime`, `Transport`, `TransportSource`, `TransportError`; `ClientOptions`; `RuntimeInfo`; connection, capability checks, shutdown | Runtime/deployment |
| `SessionOptions`, `ResumeOptions`, `Context`, `Reasoning`; `Client::run`, session create/resume, `Session` lifecycle | Sessions/runs |
| `RunOptions`, `RunHandle` (including `Stream`), `RunResult`, `RunStatus`, `Usage`, `Event`, `EventEnvelope` | Sessions/runs |
| `ExecutionPolicy`, `SandboxMode`, all three `McpServerConfig` transports; runtime-mediated delegation | Permissions/MCP |
| `SessionSummary`, `AttachedSession`; hosted session list/attach/detach | Hosted sessions/work/approvals |
| `WorkListed`, `WorkItemWire`, `WorkCountsWire`, `WorkChanged`, `WorkWatcher`, every `WorkActionKind` | Hosted sessions/work/approvals |
| `ApprovalStream`, `ApprovalRequest`, `ApprovalRequested`, `ApprovalDecision`, `ApprovalOutcome` | Hosted sessions/work/approvals |
| `Error`, `Result`, `RuntimeDiagnostics`; protocol/version constants and `protocol` re-export | Runtime/deployment |

### Do not invent unsupported SDK methods

There is no built-in SSH/network connector, direct Rust method for each agent tool, provider-login API, historical-transcript API, durable-session deletion API, or `spawn_agent()` method. Delegation happens through the runtime's agent tool when available and permitted. Text prompts do not imply a typed image/file attachment API.

The public `protocol` module also contains wire-only question-answer, run-drain, and session-ownership commands. They have no high-level `Client` wrapper in this SDK version. Do not suggest a nonexistent `client.send(...)`, equate observation with ownership transfer, or bypass the client's single event reader. A new protocol client/adapter is a separate design task, not an ordinary SDK recipe.

## Verification and handoff

- Compile against the user's pinned SDK, not an assumed latest API. Use only dependencies present in the application or explicitly add the ones required by the chosen example.
- Test routing, final status, limits, cancellation races, missing capabilities, malformed output, cleanup, and approvals as appropriate. A fake transport needs no provider key; real-runtime tests and real-provider tests are different checks.
- For SDK-repository changes, use `cargo fmt --all -- --check`, `cargo test --workspace --all-targets`, `cargo test --workspace --doc`, and `cargo clippy --workspace --all-targets -- -D warnings`; with a runtime, also `scripts/run-examples.sh <path-to-o4-agent-runtime>`.
- Verify a real runtime handshake when available. Do not count a skipped test as runtime verification, or compilation as proof an MCP server or provider works.
- Report what was checked and any untested runtime/provider-dependent behavior. Do not commit, publish, install services, spend provider credits, or change permission policy without the user's authorization.
