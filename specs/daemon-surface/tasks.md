# daemon-surface — Tasks

Four phases, in dependency order. Each is independently shippable: the repo is
green and the SDK is usable at the end of every phase. Stopping after any phase
leaves a coherent product, which is the point of splitting them.

Every phase ends with the same gate, per `AGENTS.md`:

```
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

---

## Phase 1 — Sync the protocol crate (R1)

No public API change. This is the foundation; nothing else can start.

- **T-1.1** Copy o4's `crates/o4-agent-protocol/src/lib.rs` over the SDK's copy.
  Verify `cmp` reports identical. Build.
  → *Done when:* `cmp` is silent and the workspace compiles.

- **T-1.2** Add the drift guard (D1): a test comparing a committed digest of the
  authoritative file against the local copy, recording the o4 commit the digest
  came from. The failure message names that commit.
  → *Done when:* the test passes; mutating one byte of the protocol file fails
  it with a message naming the commit to sync from. **Prove the failure**, then
  restore — a guard that has never been seen failing is not a guard.

- **T-1.3** Confirm `PROTOCOL_VERSION` is still `1` and `SDK_CAPABILITIES` is
  unchanged (R1.3).

- **T-1.4** Check whether any existing test asserted on `Event::Unknown` for
  traffic that is now typed (design: *What could go wrong*). Update, and note
  the behavior change for the release note.
  → *Done when:* the full suite passes and any such test is knowingly updated,
  not silently deleted.

## Phase 2 — Capabilities, listing, attach (R2, R3)

First public API change → **SDK version bump** (R7.1).

- **T-2.1** Expose the runtime's advertised daemon capabilities (R2.2).

- **T-2.2** Add the `MissingCapability` error variant carrying the capability
  name (D6), and the pre-wire check that raises it (R2.1, D5).
  → *Done when:* a call needing an unadvertised capability fails without
  sending a frame. Assert on the transport fake that nothing was written.

- **T-2.3** `Client::list_sessions` (R3.1), surfacing `cwd` (R3.6).

- **T-2.4** `Client::attach_session` → `AttachedSession` with `next_event`
  (R3.2), and `detach` (R3.3).

- **T-2.5** Detach on drop (design: *Attach leaks*).
  → *Done when:* a test drops an attachment without calling `detach` and
  observes the runtime stop fanning events.

- **T-2.6** Prove attaching does not widen policy (R3.4) and detaching does not
  disturb the session (R3.5).
  → *Done when:* a test attaches to a session with a restrictive policy,
  detaches, and shows the session still running under the original policy.

- **T-2.7** Bump the SDK version; update `VERSIONING.md` and `compatibility.toml`
  per R7.3; confirm `compatibility_pin_matches_compiled_default` still passes.

## Phase 3 — Work registry (R4)

Additive. Useful alone: this is what a dashboard consumes.

- **T-3.1** `Client::list_work` (R4.1).
- **T-3.2** `Client::act_on_work` covering all five actions (R4.2).
- **T-3.3** Deliver work-change events (R4.3).
- **T-3.4** Unknown `work_id` fails with a named error (R4.4).
  → *Done when:* acting on a removed row errors rather than reporting success.

## Phase 4 — Approvals and questions (R5)

The largest phase and the only bidirectional flow. **The two halves land
together** (D4): shipping `interactive_approvals` without the answering path
turns a working headless session into a silent hang.

- **T-4.1** `Client::approvals()` → `ApprovalStream` yielding approval and
  question requests, each answerable by id (R5.2, D3).

- **T-4.2** Add `interactive_approvals` to `ExecutionPolicy` and the
  `SessionOptions` builder, defaulting false (R5.5).
  → *Constraint:* this task does not land without T-4.1.

- **T-4.3** Require the `interactive_approvals` capability at session creation
  when the flag is set (R5.3).

- **T-4.4** Handle already-resolved answers as a normal outcome, not a failure
  (R5.4).
  → *Done when:* two clients answer the same request and the loser sees a
  resolved result rather than an error.

- **T-4.5** End-to-end against a real runtime: create a session with approvals
  on, receive a request, answer it, watch the run finish.
  → *Done when:* the run completes. This is the requirement's acceptance
  criterion 3 and cannot be met by a fake alone.

- **T-4.6** Regression: the same application with approvals off behaves exactly
  as before (R5.5).

## Phase 5 — Drain and ownership (R6)

Least urgent. Worth deferring until something drives handoff through the SDK.

- **T-5.1** `Client::drain_run`, with and without `now` (R6.1).
- **T-5.2** `Client::report_ownership` (R6.2).
- **T-5.3** Both gated on their capabilities (R6.3).

---

## Verification against a real runtime

Phases 2 through 5 each need a runtime advertising the relevant capability. The
SDK's existing e2e test already spawns the installed `o4-agent-runtime`, so the
pattern exists. Before starting a phase, confirm the runtime on the machine
advertises what that phase needs — a phase cannot be verified against a runtime
that does not implement it, and the pin should record the version actually
exercised (R7.3).

## Not doing

- Changing the wire protocol. This is v1 throughout.
- Implementing daemon behavior in o4; it exists, specified by `daemon-core`.
- A convenience callback wrapper for approvals. Layerable later without a
  breaking change (D3); not needed to close the gap.
- Paging `list_sessions`. o4's list is a single query and is not paged.
