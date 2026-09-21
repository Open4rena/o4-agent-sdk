# daemon-surface — Requirements

## Summary

The runtime hosts sessions, tracks their work in a registry, fans permission
requests out to attached clients, and lets a client attach to a session it did
not create. The SDK can reach none of it. Its copy of `o4-agent-protocol` is
583 lines behind o4's: eight commands and nine events exist on the wire and are
absent from the SDK, so an application that links the SDK can start a session
and stream its run, but cannot see what else the runtime is doing, cannot
answer a permission prompt, and cannot attach to a session another process
started.

Nothing is broken today. Unknown events deserialize to `Event::Unknown` with
their fields intact, and the SDK's `ExecutionPolicy` has no
`interactive_approvals` field, so an SDK-created session cannot ask a question
the SDK cannot answer. The gap is capability, not correctness — which is why
this is a feature spec and not a bug fix.

The cost of leaving it is that the SDK is stuck describing a single-session,
single-client world the runtime outgrew. Any application that wants a dashboard
of hosted sessions, an approval prompt, or a second client attaching to a live
run has to bypass the SDK and speak JSONL itself, which defeats the point of
shipping a control plane.

## Scope

In scope: syncing `o4-agent-protocol` with o4; a typed SDK surface for session
listing, attach/detach, the work registry, approvals and questions, run drain,
and ownership reporting; the capability negotiation and version pin that gate
them.

Out of scope: changing the wire protocol (this is protocol v1 throughout);
implementing daemon behavior in the runtime (it already exists, specified by
`daemon-core` in the o4 repository); a CLI; any change to how the SDK's
existing single-session API behaves; transport changes.

## Glossary

- **Daemon surface** — the commands and events added to protocol v1 for hosted,
  multi-client operation: `SessionList`, `SessionAttach`, `SessionDetach`,
  `WorkList`, `WorkAction`, `ApprovalRespond`, `QuestionAnswer`, `RunDrain`,
  `SessionOwnership`, and the events answering them.
- **Attached client** — a connection subscribed to a hosted session's events,
  whether or not it created the session.
- **Hosted session** — a session the runtime holds in memory and will serve to
  any client that attaches, as opposed to one owned by the connection that
  created it.
- **Work registry** — the runtime's record of in-flight work (runs, tool calls,
  sub-agents), each row carrying a state such as `needs_input`.
- **Parked run** — a run blocked on a permission request or question with no
  client answering it. It does not fail and does not time out.
- **Capability** — a string the runtime advertises at handshake
  (`session_list`, `work_registry`, `interactive_approvals`, …) declaring that
  it implements a slice of the daemon surface.

---

## R1 — The protocol crate is one artifact, not two copies

- **R1.1** THE SDK's `o4-agent-protocol` SHALL be byte-identical to the o4
  repository's copy, as `AGENTS.md` already requires.
- **R1.2** A check SHALL fail when the two copies differ, and SHALL name the
  o4 commit the SDK copy corresponds to. Today the invariant is stated in prose
  and nothing enforces it, which is how 583 lines of drift accumulated
  unnoticed.
- **R1.3** Syncing the crate SHALL NOT change `PROTOCOL_VERSION`, which stays
  `1`. Every addition in the drift is additive within v1: 583 lines added, none
  removed or altered.
- **R1.4** Syncing SHALL NOT by itself change the SDK's public API. A consumer
  that never touches the daemon surface SHALL compile and behave identically
  before and after.

## R2 — The daemon surface is opt-in and capability-gated

- **R2.1** WHEN the runtime does not advertise the capability backing a call,
  THE SDK SHALL fail that call with a distinct, named error before sending
  anything on the wire, rather than sending a command the runtime will reject.
- **R2.2** THE SDK SHALL expose which daemon capabilities the connected runtime
  advertises, so an application can degrade deliberately instead of probing by
  trial and error.
- **R2.3** An application that uses none of the daemon surface SHALL NOT be
  required to handle any of its events, errors, or states.
- **R2.4** Requiring a daemon capability at connect time SHALL remain possible
  through the existing `ClientOptions::require_capability`, so an application
  that cannot degrade fails at the handshake rather than mid-run.

## R3 — Listing and attaching to hosted sessions

- **R3.1** THE SDK SHALL list hosted sessions, returning for each at least the
  fields `SessionSummary` carries on the wire, including `cwd`.
- **R3.2** THE SDK SHALL attach to a hosted session by id and deliver that
  session's events to the caller.
- **R3.3** THE SDK SHALL detach from an attached session, after which its events
  SHALL stop being delivered and the session SHALL remain hosted and untouched.
- **R3.4** Attaching SHALL NOT widen the session's `ExecutionPolicy`. The policy
  a session was created with governs it for the session's whole life, per
  `daemon-core` R3.4.
- **R3.5** A dropped attachment SHALL NOT cancel, close, or otherwise disturb
  the session. Detaching is a view operation.
- **R3.6** THE SDK SHALL surface the session's recorded `cwd` on every listing.
  A caller choosing a session to attach to is choosing a working directory, and
  the resume-directory work in o4 established that a session's directory is
  part of its identity rather than an incidental detail.

## R4 — The work registry

- **R4.1** THE SDK SHALL snapshot the work registry, returning each row's id,
  state, owning session, and counts as the wire types carry them.
- **R4.2** THE SDK SHALL act on a registry row with each action the wire
  supports: stop, message, foreground, rename, clear.
- **R4.3** THE SDK SHALL deliver work-registry change events to an interested
  caller, so a dashboard can follow state without polling.
- **R4.4** An action naming a row that no longer exists SHALL fail with a named
  error rather than silently succeeding.

## R5 — Approvals and questions

- **R5.1** THE SDK SHALL expose `interactive_approvals` on `ExecutionPolicy`
  ONLY together with a way to answer the resulting requests. A session created
  with approvals on and no answering client parks its run indefinitely — it
  does not fail and does not time out (`daemon-core` R3.3, R4.3) — so shipping
  the flag alone would convert a working headless session into a silent hang.
- **R5.2** THE SDK SHALL deliver `ApprovalRequested` and `QuestionAsked` to the
  caller and SHALL let the caller answer each by request id.
- **R5.3** WHEN a caller enables `interactive_approvals`, THE SDK SHALL require
  the `interactive_approvals` capability at the point the session is created,
  and SHALL fail there if the runtime does not advertise it.
- **R5.4** THE SDK SHALL accept that another attached client may answer first.
  A late answer SHALL be reported as already-resolved, not as an error the
  application must handle as a failure (`daemon-core` R3.3).
- **R5.5** THE SDK's default SHALL remain approvals-off. An existing application
  that is recompiled against the new SDK SHALL NOT start prompting.

## R6 — Run drain and ownership

- **R6.1** THE SDK SHALL request a run drain for a session, with and without the
  `now` flag, and SHALL report when the drain completes.
- **R6.2** THE SDK SHALL report a committed ownership change for a hosted
  session, carrying the owner and generation the wire requires.
- **R6.3** These SHALL be available only when the runtime advertises
  `run_drain` and `session_ownership` respectively, per R2.1.

## R7 — Versioning and compatibility

- **R7.1** Adding this surface changes the SDK's public API, so the SDK version
  SHALL be bumped, per `VERSIONING.md`.
- **R7.2** `PROTOCOL_VERSION` SHALL remain `1` and the runtime pin SHALL NOT
  narrow as a side effect of this work.
- **R7.3** The verified `runtime_version` SHALL be updated to the runtime this
  surface was actually exercised against, and
  `compatibility_pin_matches_compiled_default` SHALL keep the pin and the
  compiled constant in step.
- **R7.4** An older runtime that advertises none of these capabilities SHALL
  remain usable for everything the SDK does today, failing only the daemon calls
  themselves, per R2.1.

## Acceptance

The spec is satisfied when:

1. `cmp` of the two `o4-agent-protocol/src/lib.rs` files is byte-identical and a
   test enforces it.
2. An application can list hosted sessions, attach to one it did not create,
   read its events, and detach without disturbing it.
3. An application can enable `interactive_approvals`, receive a request, answer
   it, and see the run continue — and the same application with approvals off
   behaves exactly as it does today.
4. An SDK built against a runtime advertising no daemon capabilities still
   creates sessions, runs, cancels, steers, and resumes.
5. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
   warnings`, and `cargo test --workspace --all-targets` all pass.
