# Hosted-session observation, work dashboards, and human approvals

Applies to o4 Agent SDK v0.4.1. These high-level APIs require capabilities advertised by the **connected runtime**. They are not proof that a daemon is installed/enabled, that a particular daemon endpoint exists, or that a second `LocalRuntime` can see the first process's live sessions.

## Negotiate support first

| Workflow | Required capability |
|---|---|
| `Client::list_sessions` | `session_list` |
| `Client::attach_session`, attachment `detach` | `session_attach` |
| `Client::list_work`, `watch_work` | `work_registry` |
| `Client::act_on_work` | `work_action` |
| `Client::approvals`, approval-enabled session creation | `interactive_approvals` |

Use `o4_agent_sdk::protocol::capabilities` constants, `ClientOptions::require_capability(...)`, or inspect `client.runtime_info().capabilities`. `client.daemon_capabilities()` returns the recognized subset of these five capabilities. It does **not** report every protocol capability or enable anything.

A required feature missing at connection yields `Error::MissingCapabilities`; a gated method missing support yields `Error::MissingCapability` before it sends the command. Offer an accurate unsupported-feature state or fail explicitly; do not guess a daemon URL, weaken policy, or silently return an empty dashboard.

`LocalRuntime` launches a new process each time. The SDK includes no built-in daemon socket/SSH connector. A connection to an existing shared host needs an application-provided `Transport`/`TransportSource` and a supported endpoint; see [Custom transports](transports-and-deployment.md). Public users should not be directed to private o4 source to obtain such an adapter.

## Discover and observe hosted sessions

`list_sessions()` returns sessions currently hosted by that runtime, including ones not created by this client. It is **not** a list of every saved session on disk. Each `SessionSummary` includes ID, recorded `cwd`, optional UUID/model/title, creation/activity timestamps, optional active run, attached-client count, and ownership metadata. Present the directory as part of session identity, and let the user choose; do not attach to the first session arbitrarily.

```rust,no_run
use o4_agent_sdk::{Client, Event, SessionSummary};

async fn available_sessions(client: &Client) -> o4_agent_sdk::Result<Vec<SessionSummary>> {
    client.list_sessions().await
}

async fn observe_one_event(client: &Client, chosen_id: &str) -> o4_agent_sdk::Result<()> {
    let mut attached = client.attach_session(chosen_id).await?;
    println!("observing: {}", attached.id());
    if let Some(envelope) = attached.next_event().await {
        match envelope.event {
            Event::MessageUpdated(_) | Event::MessageCompleted(_) => {
                // Replace the application's displayed message snapshot here.
            }
            _ => {}
        }
    }
    attached.detach().await
}
```

For a continuous UI, loop `next_event()` with the application's disconnect/stop signal and detach when done. It can wait indefinitely when the session is idle; the one-event example does not promise a timeout or transcript replay. `AttachedSession::next_event()` returns `Option<EventEnvelope>`, unlike the fallible `RunHandle::next_event()`.

Attachment is observation: no ownership transfer, policy escalation, resume, or `run()` method. `detach(self)` leaves the session running. Dropping the handle performs a best-effort detach; explicit detach lets you check its result. A slow observer can lose updates in its bounded queue, so do not call this a lossless audit log. Render safely: model/tool content and session titles are untrusted data.

## Work snapshots, change notices, and actions

`list_work()` returns `WorkListed`:

- `revision`: snapshot revision.
- `counts`: `needs_input`, `active`, `finished`.
- `items`: `WorkItemWire` rows containing identity/name/kind/status/owner, optional parent, elapsed milliseconds, token/tool counts, optional current action, pending input, detail, and stalled duration.
- `truncated`: the runtime capped the row list; the dashboard must not claim the list is exhaustive.

`watch_work()` returns the first snapshot plus a `WorkWatcher`. Each `next_change()` gives a `WorkChanged` revision hint; call `list_work()` again to obtain a current snapshot. Notices can be dropped when the watcher falls behind. This is **not** a replayable history/event log; coalesce refreshes instead of assuming every transition is delivered. `WorkWatcher::next_change()` returns `Option<WorkChanged>`.

```rust,no_run
use o4_agent_sdk::{Client, WorkListed};

async fn snapshot_then_refresh(client: &Client) -> o4_agent_sdk::Result<WorkListed> {
    let (initial, mut watcher) = client.watch_work().await?;
    match watcher.next_change().await {
        Some(_) => client.list_work().await,
        None => Ok(initial),
    }
}
```

Use explicit user-selected IDs and authorize actions in the application before calling `act_on_work`. Its `WorkActionKind` variants are:

| Action | Purpose |
|---|---|
| `Stop` | Request stopping the selected work |
| `Message { text }` | Send an instruction/message to selected work |
| `Foreground` | Request the runtime's foreground action |
| `Rename { name }` | Rename the selected row |
| `Clear` | Request removal of an eligible row from the registry |

Applicability depends on the runtime and row kind/state. Do not claim every action works for every row, that `Clear` deletes durable sessions, or that `Foreground` attaches a terminal. The call returns the actioned work ID; re-list to see the resulting state. A stale ID can return runtime code `work_not_found` rather than success; show that and refresh instead of retrying against another row.

```rust,no_run
use o4_agent_sdk::{Client, WorkActionKind};

async fn rename_selected(client: &Client, work_id: &str, name: &str) -> o4_agent_sdk::Result<String> {
    client
        .act_on_work(work_id, WorkActionKind::Rename { name: name.into() })
        .await
}
```

## Answer approval requests without parking the agent

Enable interactive approvals only after deciding with the user how requests are presented, who can answer, what happens on disconnect, and what timeout/deny behavior the application needs. Never implement "approve everything" as a convenience fallback. Request summaries are untrusted content, not instructions to the application or a trusted authorization decision.

1. Require `interactive_approvals` at connection (or handle `MissingCapability`).
2. Take `client.approvals().await?` **once per connection**, before creating the session. Taking it a second time is an error; clones do not create independent approval streams.
3. Continuously run an approval reader **alongside** run/event consumption. Waiting for `final_result()` before starting the reader can deadlock the workflow.
4. Create the session with `ExecutionPolicy { interactive_approvals: true, ..existing_policy }`, preserving the agreed static restrictions.
5. Present the request through the application's authenticated decision path and call `ApprovalRequest::respond` with that decision. Use application-level timeouts/denials for abandoned dialogs; the SDK does not supply a user-response timeout.
6. Keep servicing requests, including child requests, until work ends. Close/clean up owned sessions on failure or disconnect, then explicitly stop/join the handler. Do not rely on runtime shutdown to close the approval stream: its sender may remain alive while SDK handles exist. An unanswered request may park a run indefinitely.

The following helper receives a real application decision callback. It does **not** decide policy or automatically approve anything. `show_and_decide` must validate the responder and return a decision within the application's deadline. Run this helper concurrently with the session workflow, not before it in a blocking sequence. A session workflow may preserve the chosen policy as shown here:

```rust
use o4_agent_sdk::ExecutionPolicy;

fn with_human_approvals(policy: ExecutionPolicy) -> ExecutionPolicy {
    ExecutionPolicy { interactive_approvals: true, ..policy }
}
```

```rust,no_run
use std::future::Future;
use o4_agent_sdk::{ApprovalDecision, ApprovalOutcome, ApprovalRequest, ApprovalStream};
use tokio::sync::oneshot;

async fn answer_approvals<F, D>(
    mut stream: ApprovalStream,
    stop: oneshot::Receiver<()>,
    mut show_and_decide: F,
) -> o4_agent_sdk::Result<()>
where
    F: FnMut(ApprovalRequest) -> D,
    D: Future<Output = (ApprovalRequest, ApprovalDecision)>,
{
    let serve = async {
        while let Some(request) = stream.next_request().await {
            let (request, decision) = show_and_decide(request).await;
            if decision == ApprovalDecision::AlwaysAllow && !request.always_allow_available() {
                return Err(o4_agent_sdk::Error::ProtocolViolation {
                    reason: "the selected persistent approval is not offered".into(),
                });
            }
            match request.respond(decision).await? {
                ApprovalOutcome::Recorded => {}
                ApprovalOutcome::AlreadyResolved => {
                    // Another client answered, or the request is no longer pending.
                }
            }
        }
        Ok(())
    };
    tokio::select! {
        result = serve => result,
        _ = stop => Ok(()),
    }
}
```

Keep the sender returned by `oneshot::channel()` in the workflow owner and pass its receiver to the handler. After work completes or owned sessions have been cleaned up, send the stop signal and join the handler. This explicit stop is needed for **owned and shared** runtimes: `Client::shutdown()` does not guarantee `next_request()` returns `None` while SDK handles retain the sender. The stop branch also drops an in-progress decision/response future, so only stop after pending work is resolved/closed, or let another responsible handler take over. Never abandon pending approvals on an active shared session. If the handler exits with an error first, stop the workflow and clean up instead of leaving it parked; do not shut down other users' runtime to end this task.

`ApprovalRequest` exposes `id`, `tool`, `summary`, optional `work_id`, `always_allow_available`, and full `payload()` (including optional risk). Do not invent session/user identifiers on this handle; correlate through available work/session records and the application's authenticated connection. Enforce which sessions/users a responder may approve outside SDK metadata.

Decisions are `Approve`, `Deny`, and `AlwaysAllow`. Offer the latter **only** when available and explicitly chosen; it remembers approval for the remainder of the session where the runtime permits it. `Recorded` means the answer was accepted, not that the tool succeeded. `AlreadyResolved` is a normal race with another client, not a reason to submit again. Keep readers responsive; approval delivery is bounded and must not be described as an unlimited/lossless queue.

## Wire-only operations are not SDK recipes

The re-exported `protocol` types include `QuestionAsked`/`QuestionAnswer`, `RunDrain`/`RunDrained`, and ownership-reporting types/commands. SDK v0.4.1 has no high-level question-response, handoff/drain, or ownership-transfer methods. Observing an event or reading ownership metadata does not implement those workflows. Do not inject commands behind `Client`, add a second raw reader, or claim a handoff feature based only on the presence of a wire enum. A supported adapter/protocol extension needs its own design and verification.
