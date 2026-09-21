# daemon-surface — Design

## The shape of the problem

The SDK models one client owning one session at a time. `Client::create_session`
returns a `Session`, `Session::run` returns a `RunHandle`, and events flow back
through that handle. The daemon surface breaks three assumptions in that model
at once:

1. **Sessions outlive the connection that made them.** `SessionList` returns
   sessions this client never created and may not own.
2. **Events are not owned by a run handle.** `ApprovalRequested`,
   `WorkChanged`, and `QuestionAsked` arrive for a session, not for a run the
   caller is holding, and may arrive for a session this client merely attached
   to.
3. **Answering is a request the runtime makes of the application**, inverting
   the direction every existing SDK call runs in.

The third is the one that shapes the design. Everything else is a typed
request/response pair over machinery the SDK already has.

## Decisions

### D1 — Sync the protocol crate by copy, enforced by test

`AGENTS.md` already requires the two copies be byte-identical and calls the o4
copy authoritative. The drift happened because nothing checked. So: copy o4's
file wholesale, and add a test that fails when they differ.

The alternative — a shared crate published to a registry, or a path dependency
across repositories — is a build-system change that makes the SDK unbuildable
without an o4 checkout. That trades a detectable problem for a structural one.
Rejected.

The enforcing test must work in a repository that has no o4 checkout beside it.
It therefore compares against a committed digest of the authoritative file, and
records the o4 commit that digest came from, rather than reading across the
filesystem. When they diverge, the failure names the commit to sync from.

### D2 — Daemon calls hang off `Client`, not `Session`

`SessionList`, `WorkList`, `WorkAction`, and the approval answers are
connection-scoped, not session-scoped: they concern sessions this client may not
own. Putting them on `Client` matches the wire. Putting them on `Session` would
require inventing a `Session` for rows the caller has not attached to.

`SessionAttach` returns something `Session`-shaped, because from that point the
caller does have a session. Attachment and ownership differ in what they permit,
not in what they are.

### D3 — Approvals are a stream the application drains, not a callback

Two shapes were considered.

**A callback** — `ClientOptions::on_approval(|req| ...)` — reads well for simple
cases and is what many SDKs do. It also forces a decision about which executor
runs the callback, what happens when it panics, how it is cancelled, and whether
it may borrow application state across an await. Every one of those is a trap.

**A stream** — the application receives requests and answers by id — keeps the
SDK free of executor policy, matches `RunHandle::next_event` which applications
already drive, and lets an application that wants a callback write the three
lines that wrap it. It cannot deadlock the runtime, because not draining is
indistinguishable from having no attached client, which the runtime already
handles by parking.

Chosen: the stream. An ergonomic callback helper can be layered later without a
breaking change; going the other way is not possible.

### D4 — `interactive_approvals` ships with its answering path or not at all

This is the requirement most likely to be shortcut under time pressure, so it is
called out as a design decision rather than left in the requirements.

The field is one `bool` on a struct the SDK already sends. Adding it is a
five-minute change, and it is exactly the change that must not land alone:
a session created with `interactive_approvals: true` and no client answering
parks forever. Not fails — parks. `daemon-core` R3.3 makes that explicit, and
R4.3 marks the registry row `needs_input` and waits.

So an application that flipped the flag expecting "now I get prompts" would
instead get a run that never completes and never errors. The flag and the
answering path are one unit of work.

### D5 — Capability checks happen before the wire, and name what is missing

R2.1 could be satisfied by sending the command and translating the runtime's
rejection. Checking locally against the advertised set is better: the error can
name the capability and the runtime version, it costs no round trip, and it
cannot be confused with a transport failure.

The handshake already records the advertised set. This is a lookup.

### D6 — One new error variant, not one per capability

`Error` gains a single variant carrying the missing capability's name, rather
than seven variants. Applications match on the variant and inspect the name;
the alternative grows the enum every time the runtime gains a capability, and
each addition is a breaking change for anyone matching exhaustively.

## Surface sketch

Illustrative, not final. Names are the reviewable part; signatures will follow
the existing style.

```
Client
  daemon_capabilities() -> &[String]          // R2.2
  list_sessions() -> Vec<SessionSummary>      // R3.1
  attach_session(id) -> AttachedSession       // R3.2
  list_work() -> WorkSnapshot                 // R4.1
  act_on_work(work_id, WorkAction)            // R4.2
  approvals() -> ApprovalStream               // R5.2, D3
  drain_run(session_id, now)                  // R6.1
  report_ownership(session_id, owner, gen)    // R6.2

AttachedSession
  id()
  next_event()                                 // R3.2
  detach()                                     // R3.3

ApprovalStream
  next() -> ApprovalRequest | QuestionRequest  // R5.2
  // each carries respond(decision) / answer(text), by request id
```

`ExecutionPolicy` gains `interactive_approvals: bool`, defaulting false (R5.5),
and `SessionOptions` gains the builder that sets it — landing together with
`approvals()` per D4.

## What could go wrong

- **The protocol sync pulls in a runtime-only type.** o4's copy is shared by
  construction, but if a type there depends on something outside the protocol
  crate the sync will not compile. Mitigation: the sync is mechanical and
  verified by building; if it does not build, the invariant was already broken
  and that is the finding.
- **`Event::Unknown` stops matching.** Applications that today receive daemon
  events as `Unknown` will, after the sync, receive them as typed variants. An
  application matching `Unknown` to ignore them still ignores them via its
  catch-all arm, but one that matched `Unknown` *expecting* daemon traffic
  changes behavior. This is the one observable change for existing code and
  belongs in the release note.
- **Attach leaks.** An attachment that is dropped without detaching leaves the
  runtime fanning events at a gone client. The SDK should detach on drop, and
  the tests should prove a dropped attachment stops traffic.

## Open question for review

**How much of this should land at once?** The requirements describe the whole
surface, but it splits cleanly:

- **Sync + listing + attach** (R1, R2, R3) is self-contained, is the foundation
  everything else needs, and carries no behavioral risk to existing consumers.
- **Work registry** (R4) is additive on top and useful alone — it is what a
  dashboard needs.
- **Approvals** (R5) is the largest piece, the only bidirectional flow, and the
  one that changes `ExecutionPolicy`.
- **Drain and ownership** (R6) are two commands serving handoff, and are the
  least urgent unless something is actually driving handoff through the SDK.

My recommendation is to land them in that order as separate changes, with a
version bump on the first one that changes the public API. Doing all four at
once means one large review of a surface nobody has exercised yet.

The tasks file is written assuming that order, with each phase independently
shippable.
