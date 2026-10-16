# Sessions, runs, structured jobs, and live control

Applies to o4 Agent SDK v0.4.1. Examples use `o4_agent_sdk` and Tokio; the schema example additionally needs `serde_json = "1"`. Connection/setup and error diagnostics are in [Runtime and deployment](transports-and-deployment.md).

## Choose a lifecycle

| Need | API | Responsibility |
|---|---|---|
| One prompt, final result | `Client::run(prompt, SessionOptions)` | SDK creates and unloads the session, even when running fails. It does not delete durable data or shut down the client. |
| Multiple prompts or custom run options | `Client::create_session` then `Session::run` / `run_with_options` | Application owns the session and closes it. |
| Continue after restart | `Client::resume_session(id, ResumeOptions)` | Use the same durable state location and original workspace. |
| Live progress and control | `RunHandle::next_event` or its `Stream` implementation | Continuously consume events; finish with `final_result`. |

A `Client` clone shares the connection/router; it is not a second runtime. A cloned `Session` refers to the same session and shared closed state. Use separate sessions for independent concurrent jobs; do not start overlapping runs in one session.

## One-shot task

This helper borrows an already connected client. Its owner must call `shutdown()` after the helper succeeds **or fails**. Pick a configured model and an existing project directory; default permissions do not authorize arbitrary writes or commands.

```rust,no_run
use std::path::Path;
use o4_agent_sdk::{Client, RunResult, SessionOptions};

async fn summarize(client: &Client, model: &str, cwd: &Path) -> o4_agent_sdk::Result<RunResult> {
    client
        .run("Summarize this workspace", SessionOptions::new(model, cwd))
        .await
}
```

## Durable, multi-turn conversations

`SessionOptions::new(model, cwd)` selects the model and workspace. Additional builders are `session_id`, `system_prompt`, `reasoning`, `execution_policy`, `mcp_server` (repeatable), and `context`. Its fields are also public. Let the runtime assign an ID unless the application has a collision-safe ID scheme.

`Reasoning::{Minimal, Low, Medium, High}` requests an effort level; actual support depends on the model. Do not change the user's model/effort without a reason and approval.

`Context` carries optional `workspace_id`, `user_id`, and `correlation_id`. It is event metadata, not conversation content or an access-control boundary. A system prompt is guidance, not a substitute for execution policy.

```rust,no_run
use std::path::Path;
use o4_agent_sdk::{Client, Context, ResumeOptions, RunResult, SessionOptions};

async fn begin(
    client: &Client,
    model: &str,
    cwd: &Path,
    context: Context,
) -> o4_agent_sdk::Result<(String, RunResult)> {
    let session = client
        .create_session(
            SessionOptions::new(model, cwd)
                .system_prompt("Be concise and explain your findings.")
                .context(context),
        )
        .await?;
    let id = session.id().to_owned();
    let outcome = async {
        let first = session.run("Describe the project").await?.final_result().await?;
        if first.status != o4_agent_sdk::RunStatus::Completed {
            return Ok(first);
        }
        session.run("Which parts need tests?").await?.final_result().await
    }
    .await;
    let closed = session.close().await;
    let result = outcome?;
    closed?;
    Ok((id, result))
}

async fn continue_conversation(
    client: &Client,
    id: &str,
    original_cwd: &Path,
    context: Context,
) -> o4_agent_sdk::Result<RunResult> {
    let session = client
        .resume_session(id, ResumeOptions::default().cwd(original_cwd).context(context))
        .await?;
    let outcome = async {
        session.run("Continue from our previous findings").await?.final_result().await
    }
    .await;
    let closed = session.close().await;
    let result = outcome?;
    closed?;
    Ok(result)
}
```

Persist the returned ID through the application's storage, then shut down the first owned runtime. Connect another runtime with the **same** `LocalRuntime::data_dir` and call the resume helper. Keep provider setup and the original workspace available too.

`Session::close()` unloads the session; saved state remains resumable. `ResumeOptions::cwd` **asserts** the recorded directory and fails on a mismatch: it cannot move the session. Omitting it accepts the saved directory. Resume options expose only `cwd` and `context`; they do not override the saved model, prompt, policy, or MCP definition.

## Structured output and bounded jobs

Use `Session::run_with_options`, not the one-shot helper, when a job needs `RunOptions`. Request the `structured_output` runtime capability before relying on schema output. Limit enforcement and validation still belong to the runtime; verify behavior with the chosen model/runtime.

```rust,no_run
use std::path::Path;
use o4_agent_sdk::{Client, RunOptions, RunResult, SessionOptions};
use serde_json::json;

async fn bounded_report(
    client: &Client,
    model: &str,
    cwd: &Path,
) -> o4_agent_sdk::Result<RunResult> {
    let session = client.create_session(SessionOptions::new(model, cwd)).await?;
    let outcome = async {
        let options = RunOptions::default()
            .output_schema(json!({
                "type": "object",
                "properties": {"summary": {"type": "string"}},
                "required": ["summary"],
                "additionalProperties": false
            }))
            .max_turns(8)
            .token_budget(12_000)
            .timeout_secs(120);
        session
            .run_with_options("Return a brief workspace summary", options)
            .await?
            .final_result()
            .await
    }
    .await;
    let closed = session.close().await;
    let result = outcome?;
    closed?;
    Ok(result)
}
```

Before consuming application data, check `result.status == RunStatus::Completed`, require `result.structured_output`, and deserialize/validate it for the application's own rules. Do not silently fall back to parsing prose if a schema result was required.

`RunResult` contains `status`, `output`, optional `structured_output`, `messages` (JSON values), `usage`, `turns`, and `tool_calls`. `Usage` reports `input_tokens`, `output_tokens`, `total_tokens`, `cached_input_tokens`, and optional `cost_usd`; missing cost is unknown, not necessarily free.

`RunStatus` can be `Completed`, `Cancelled`, `Failed`, `TimedOut`, `ResourceLimited`, or `Unknown`. An `Ok` API result may contain any of these. For a CI/batch application, map non-completed states to the application's failure/cancellation outcome. Bound concurrency across independent sessions and preserve each job's identity and result; retries of side-effecting jobs need an application-level decision, not an unconditional retry loop.

## Streaming, steering, and cancellation

`EventEnvelope` carries request/sequence/timestamp/runtime identity plus optional session/run IDs and context. `known_run_id()` is `None` until the run is identified; `steer()` and `cancel()` wait for that identity internally.

- `MessageStarted`, `MessageUpdated`, `MessageCompleted`: `message` is a **whole JSON snapshot**. Replace the displayed snapshot rather than concatenating it like a token delta.
- `ToolStarted`, `ToolUpdated`, `ToolCompleted`: `tool_call` is a whole JSON snapshot too; it is not a direct SDK tool-invocation API.
- Lifecycle/diagnostic events include run/agent/turn start, warnings, retries, router diagnostics, context compaction, resource limits, steering confirmation, and completion/errors.
- Preserve forward compatibility with a wildcard or explicit `Event::Unknown` handling. Do not crash because an additive event is unfamiliar.

The following minimal controller accepts user instructions from an application channel and reports control-command errors separately from the run's terminal result. It prints only event types, not potentially sensitive payloads. The caller must inspect both outputs: a completed run does not imply a requested steer/cancel succeeded.

**SDK v0.4.1 limitation:** while this controller awaits a steer/cancel acknowledgement, it cannot also call `next_event()` on the same handle. Progress continues buffering in the background. A delayed reply plus more than 256 queued events can overflow and fail the connection. This is not a fully nonblocking, high-volume control recipe. Verify acknowledgement latency/event volume with the intended runtime; an independently usable control handle would require an SDK change. Do not bypass `Client` or add a raw reader as a workaround. Outside control waits, keep rendering fast or use a bounded UI queue.

```rust,no_run
use o4_agent_sdk::{Error, Event, RunHandle, RunResult};
use tokio::sync::mpsc;

enum Control {
    Steer(String),
    Cancel,
}

async fn drive(
    mut handle: RunHandle,
    mut commands: mpsc::Receiver<Control>,
) -> (o4_agent_sdk::Result<RunResult>, Vec<Error>) {
    let mut controls_open = true;
    let mut control_errors = Vec::new();
    loop {
        tokio::select! {
            event = handle.next_event() => {
                let envelope = match event {
                    Ok(Some(envelope)) => envelope,
                    Ok(None) => break,
                    Err(error) => return (Err(error), control_errors),
                };
                println!("event: {}", envelope.event.event_type());
                if matches!(envelope.event, Event::RunCompleted(_)) {
                    break;
                }
            }
            command = commands.recv(), if controls_open => {
                let result = match command {
                    Some(Control::Steer(text)) => handle.steer(text).await,
                    Some(Control::Cancel) => handle.cancel().await,
                    None => {
                        controls_open = false;
                        continue;
                    }
                };
                if let Err(error) = result {
                    control_errors.push(error);
                    controls_open = false;
                }
            }
        }
    }
    (handle.final_result().await, control_errors)
}
```

`RunHandle` also implements `futures_core::Stream<Item = o4_agent_sdk::Result<EventEnvelope>>`; stream-extension helpers require the application's chosen futures utility dependency. `next_event()` needs no extra futures dependency. `final_result(self)` consumes the handle and drains any remaining events.

Active-run queues are bounded (256 events in this SDK). Failing to consume them can raise `EventBufferOverflow` and fail the connection shared by other jobs. Merely sleeping on or dropping the handle is not a pause/cancel operation. A cancel acknowledgement is not proof the run has reached `Cancelled`; read the terminal result, including a race where it already completed. `steer()` injects an instruction; it is not a new session or a permission change.

`Error::RunFinished` covers a control request begun after the SDK already knows the run is terminal. Completion can also race after the command is sent; the recorded runtime returns `Error::Runtime` with code `run_not_found` when cancel/steer finds no active run. That does not prove success or a terminal status by itself. The controller preserves every control error, stops accepting further commands, and drains the run outcome separately. Do not discard an unrelated runtime error or drop a usable handle just because a control request failed. A broken connection or a peer that never answers still needs application-level deadlines and cleanup.

On a controller error, the caller still owns session/runtime cleanup. Explicitly cancel an active run when appropriate and close the session; shut down only runtimes the application owns.
