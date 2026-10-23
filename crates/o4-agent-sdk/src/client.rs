use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use futures_core::Stream;
use o4_agent_protocol::{
    capabilities, ApprovalDecision, ApprovalRequested, Command, Context, Event, EventEnvelope,
    ExecutionPolicy, McpServerConfig, Reasoning, RequestEnvelope, RunCompleted, RunStatus,
    RuntimeReady, SessionSummary, Usage, WorkActionKind, WorkChanged, WorkListed, SDK_CAPABILITIES,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use semver::{Version, VersionReq};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use crate::error::{Error, Result};
use crate::transport::{Transport, TransportSource};
use crate::DEFAULT_RUNTIME_VERSION_REQUIREMENT;

const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_BUFFER_CAPACITY: usize = 256;
/// Buffer for one attachment's fan-out. An attached observer does not drive
/// the session it is watching, so a reader that falls behind must drop events
/// rather than stall the connection every other session shares.
const ATTACHMENT_EVENT_BUFFER: usize = 256;
/// Buffer for one work watcher. Notices are hints to re-list, so a slow
/// watcher drops them rather than stalling the shared connection.
const WORK_CHANGE_BUFFER: usize = 64;
/// Buffer for pending approvals. Unlike work notices these must not be
/// dropped: a request nobody answers parks its run forever, so the buffer is
/// generous and overflow is surfaced rather than silently discarded.
const APPROVAL_BUFFER: usize = 256;
/// The capabilities this SDK knows how to use, for `daemon_capabilities`.
const DAEMON_CAPABILITIES: &[&str] = &[
    capabilities::SESSION_LIST,
    capabilities::SESSION_ATTACH,
    capabilities::WORK_REGISTRY,
    capabilities::WORK_ACTION,
    capabilities::INTERACTIVE_APPROVALS,
];

/// Connection and compatibility policy.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    client_name: String,
    client_version: String,
    runtime_version_requirement: VersionReq,
    required_capabilities: Vec<String>,
    advertised_capabilities: Vec<String>,
    handshake_timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            client_name: "o4-agent-sdk".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            runtime_version_requirement: VersionReq::parse(DEFAULT_RUNTIME_VERSION_REQUIREMENT)
                .expect("the built-in runtime version requirement must be valid"),
            required_capabilities: Vec::new(),
            advertised_capabilities: SDK_CAPABILITIES.iter().map(ToString::to_string).collect(),
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        }
    }
}

impl ClientOptions {
    pub fn client_identity(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client_name = name.into();
        self.client_version = version.into();
        self
    }

    /// Configure a runtime semver constraint. This check is separate from and
    /// subordinate to protocol/capability negotiation.
    pub fn runtime_version_requirement(mut self, requirement: VersionReq) -> Self {
        self.runtime_version_requirement = requirement;
        self
    }

    pub fn require_capability(mut self, capability: impl Into<String>) -> Self {
        self.required_capabilities.push(capability.into());
        self
    }

    pub fn advertise_capability(mut self, capability: impl Into<String>) -> Self {
        self.advertised_capabilities.push(capability.into());
        self
    }

    pub fn handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }
}

/// Compatibility information negotiated with the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub runtime_version: Version,
    pub protocol_version: u32,
    pub capabilities: Vec<String>,
}

/// Options for a durable runtime session.
#[derive(Clone)]
pub struct SessionOptions {
    pub session_id: Option<String>,
    pub model: String,
    pub cwd: PathBuf,
    pub system_prompt: Option<String>,
    pub reasoning: Option<Reasoning>,
    pub execution_policy: Option<ExecutionPolicy>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub context: Option<Context>,
}

impl SessionOptions {
    pub fn new(model: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            session_id: None,
            model: model.into(),
            cwd: cwd.into(),
            system_prompt: None,
            reasoning: None,
            execution_policy: None,
            mcp_servers: Vec::new(),
            context: None,
        }
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn reasoning(mut self, reasoning: Reasoning) -> Self {
        self.reasoning = Some(reasoning);
        self
    }

    pub fn execution_policy(mut self, policy: ExecutionPolicy) -> Self {
        self.execution_policy = Some(policy);
        self
    }

    pub fn mcp_server(mut self, server: McpServerConfig) -> Self {
        self.mcp_servers.push(server);
        self
    }

    pub fn context(mut self, context: Context) -> Self {
        self.context = Some(context);
        self
    }
}

/// Options supplied while resuming durable session state.
///
/// Only `context` changes anything. A resumed session keeps
/// the working directory it was created in: that directory is recorded in the
/// immutable session definition and is what the runtime rebuilds the session
/// from, so it cannot be relocated by resuming.
#[derive(Clone, Default)]
pub struct ResumeOptions {
    /// Asserts the directory the session was created in; it does not move it.
    pub cwd: Option<PathBuf>,
    pub context: Option<Context>,
}

impl ResumeOptions {
    /// Assert the directory this session was created in.
    ///
    /// This is a guard, not an override. Resuming fails with
    /// `session_resume_failed` when the path does not match the recorded
    /// directory, so a caller that has the wrong directory is told instead of
    /// silently getting a session rooted somewhere it did not expect. Leave it
    /// unset to accept whatever directory the session was created in.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn context(mut self, context: Context) -> Self {
        self.context = Some(context);
        self
    }
}

/// Limits and schema for one run.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub output_schema: Option<Value>,
    pub max_turns: Option<u32>,
    pub token_budget: Option<u64>,
    pub timeout_secs: Option<u64>,
}

impl RunOptions {
    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    pub fn token_budget(mut self, token_budget: u64) -> Self {
        self.token_budget = Some(token_budget);
        self
    }

    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }
}

/// Buffered terminal output from a run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    pub status: RunStatus,
    pub output: String,
    pub structured_output: Option<Value>,
    pub messages: Vec<Value>,
    pub usage: Usage,
    pub turns: u32,
    pub tool_calls: u32,
}

impl From<RunCompleted> for RunResult {
    fn from(completed: RunCompleted) -> Self {
        Self {
            status: completed.status,
            output: completed.output,
            structured_output: completed.structured_output,
            messages: completed.messages,
            usage: completed.usage,
            turns: completed.turns,
            tool_calls: completed.tool_calls,
        }
    }
}

/// A connected runtime client. Clones share one reader task and request router.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("runtime_info", &*lock(&self.inner.runtime_info))
            .field(
                "shutdown_complete",
                &self.inner.shutdown_complete.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Open a transport, initialize the runtime, and negotiate compatibility.
    pub async fn connect<S>(source: S) -> Result<Self>
    where
        S: TransportSource,
    {
        Self::connect_with_options(source, ClientOptions::default()).await
    }

    pub async fn connect_with_options<S>(source: S, options: ClientOptions) -> Result<Self>
    where
        S: TransportSource,
    {
        let transport = source.open().await?;
        let inner = Arc::new(Inner::new(transport));
        Inner::start_reader(&inner);
        let client = Self { inner };

        let handshake = client.perform_handshake(&options);
        let ready = match tokio::time::timeout(options.handshake_timeout, handshake).await {
            Ok(result) => result,
            Err(_) => Err(Error::HandshakeTimeout),
        };

        match ready {
            Ok(ready) => {
                let info = negotiate_compatibility(&ready, &options)?;
                client
                    .inner
                    .negotiated_protocol
                    .store(info.protocol_version, Ordering::Release);
                *lock(&client.inner.runtime_info) = Some(info);
                Ok(client)
            }
            Err(error) => {
                client.inner.fail_all(error.clone());
                client.inner.transport.abort();
                Err(error)
            }
        }
    }

    async fn perform_handshake(&self, options: &ClientOptions) -> Result<RuntimeReady> {
        let command = Command::Initialize {
            client_name: options.client_name.clone(),
            client_version: options.client_version.clone(),
            supported_protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            capabilities: deduplicate(options.advertised_capabilities.clone()),
        };
        let envelope = self
            .request_terminal(command, PendingKind::Handshake)
            .await?;
        match envelope.event {
            Event::RuntimeReady(ready) => Ok(ready),
            other => Err(Error::ProtocolViolation {
                reason: format!(
                    "initialize terminated with `{}` instead of `runtime_ready`",
                    other.event_type()
                ),
            }),
        }
    }

    /// Return negotiated runtime information after a successful handshake.
    pub fn runtime_info(&self) -> RuntimeInfo {
        lock(&self.inner.runtime_info)
            .clone()
            .expect("a connected Client always has runtime information")
    }

    /// Daemon capabilities this runtime advertised at the handshake.
    ///
    /// Use it to decide what to offer before calling; every daemon method
    /// below fails with [`Error::MissingCapability`] rather than guessing.
    pub fn daemon_capabilities(&self) -> Vec<String> {
        let advertised = self.runtime_info().capabilities;
        DAEMON_CAPABILITIES
            .iter()
            .filter(|capability| advertised.iter().any(|entry| entry == *capability))
            .map(|capability| (*capability).to_owned())
            .collect()
    }

    fn require_capability(&self, capability: &str) -> Result<()> {
        if self
            .runtime_info()
            .capabilities
            .iter()
            .any(|entry| entry == capability)
        {
            return Ok(());
        }
        Err(Error::MissingCapability {
            capability: capability.to_owned(),
        })
    }

    /// List the sessions the runtime currently hosts.
    ///
    /// Includes sessions this client did not create. Each summary carries the
    /// session's recorded `cwd`: choosing a session to attach to is choosing a
    /// working directory, so it is part of the identity rather than a detail.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.require_capability(capabilities::SESSION_LIST)?;
        let envelope = self
            .request_terminal(Command::SessionList, PendingKind::SessionList)
            .await?;
        match envelope.event {
            Event::SessionListed(listed) => Ok(listed.sessions),
            other => Err(Error::ProtocolViolation {
                reason: format!(
                    "session_list terminated with `{}` instead of `session_listed`",
                    other.event_type()
                ),
            }),
        }
    }

    /// Attach to a hosted session and observe its events.
    ///
    /// Attaching is a view: it does not transfer ownership, and it cannot
    /// widen the session's `ExecutionPolicy` -- the policy the session was
    /// created with governs it for life.
    pub async fn attach_session(&self, session_id: impl Into<String>) -> Result<AttachedSession> {
        self.require_capability(capabilities::SESSION_ATTACH)?;
        let session_id = session_id.into();

        // Register before sending: the runtime may fan out an event for this
        // session between its reply and our next instruction, and an event
        // arriving with nowhere to go would fail the connection.
        let (sender, receiver) = mpsc::channel(ATTACHMENT_EVENT_BUFFER);
        self.inner.register_attachment(&session_id, sender);

        let attached = self
            .request_terminal(
                Command::SessionAttach {
                    session_id: session_id.clone(),
                },
                PendingKind::SessionAttach,
            )
            .await;
        if let Err(error) = attached {
            self.inner.remove_attachment(&session_id);
            return Err(error);
        }

        Ok(AttachedSession {
            inner: Arc::clone(&self.inner),
            session_id,
            events: receiver,
            detached: false,
        })
    }

    /// Snapshot the runtime's work registry: every run, tool call, and
    /// sub-agent it is tracking, with the counts behind them.
    ///
    /// Asking also subscribes this connection to [`Client::watch_work`]
    /// notifications, which is how the runtime works: it starts telling you
    /// about changes once you have shown interest in the list.
    pub async fn list_work(&self) -> Result<WorkListed> {
        self.require_capability(capabilities::WORK_REGISTRY)?;
        let envelope = self
            .request_terminal(Command::WorkList, PendingKind::WorkList)
            .await?;
        match envelope.event {
            Event::WorkListed(listed) => Ok(listed),
            other => Err(Error::ProtocolViolation {
                reason: format!(
                    "work_list terminated with `{}` instead of `work_listed`",
                    other.event_type()
                ),
            }),
        }
    }

    /// Act on one registry row.
    ///
    /// A row that no longer exists fails with [`Error::Runtime`] carrying the
    /// runtime's `work_not_found` code, rather than reporting success for work
    /// that was never touched.
    pub async fn act_on_work(
        &self,
        work_id: impl Into<String>,
        action: WorkActionKind,
    ) -> Result<String> {
        self.require_capability(capabilities::WORK_ACTION)?;
        let envelope = self
            .request_terminal(
                Command::WorkAction {
                    work_id: work_id.into(),
                    action,
                },
                PendingKind::WorkAction,
            )
            .await?;
        match envelope.event {
            Event::WorkActioned(actioned) => Ok(actioned.work_id),
            other => Err(Error::ProtocolViolation {
                reason: format!(
                    "work_action terminated with `{}` instead of `work_actioned`",
                    other.event_type()
                ),
            }),
        }
    }

    /// Follow registry changes instead of polling.
    ///
    /// Each notice carries the registry's new revision; call
    /// [`Client::list_work`] to see what moved. The runtime only sends these
    /// to connections that have asked for the list, so this calls
    /// `list_work` once to subscribe and returns that first snapshot alongside
    /// the stream.
    ///
    /// A watcher that falls behind loses notices rather than stalling the
    /// connection every session shares: the revision is a hint to re-list, not
    /// a record the caller must not miss.
    pub async fn watch_work(&self) -> Result<(WorkListed, WorkWatcher)> {
        let (sender, receiver) = mpsc::channel(WORK_CHANGE_BUFFER);
        self.inner.register_work_watcher(sender);
        let listed = self.list_work().await?;
        Ok((listed, WorkWatcher { changes: receiver }))
    }

    /// Receive the permission requests the runtime raises.
    ///
    /// Call this *before* creating a session with `interactive_approvals`, and
    /// keep draining it. A request nobody answers parks its run forever --
    /// no error, no timeout -- so a session that prompts without a reader is
    /// a session that quietly stops.
    ///
    /// There is one stream per connection: requests are buffered from connect,
    /// so nothing is lost between connecting and asking. Taking it twice is an
    /// error rather than silently splitting the requests between two readers.
    pub async fn approvals(&self) -> Result<ApprovalStream> {
        self.require_capability(capabilities::INTERACTIVE_APPROVALS)?;
        let receiver = self
            .inner
            .approval_receiver
            .lock()
            .await
            .take()
            .ok_or_else(|| Error::ProtocolViolation {
                reason: "the approval stream was already taken".into(),
            })?;
        Ok(ApprovalStream { requests: receiver })
    }

    /// Create a session, run one prompt to completion, then unload the session.
    pub async fn run(
        &self,
        prompt: impl Into<String>,
        options: SessionOptions,
    ) -> Result<RunResult> {
        let session = self.create_session(options).await?;
        let result = match session.run(prompt).await {
            Ok(handle) => handle.final_result().await,
            Err(error) => Err(error),
        };
        let close_result = session.close().await;
        match (result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(result), Ok(())) => Ok(result),
        }
    }

    pub async fn create_session(&self, options: SessionOptions) -> Result<Session> {
        // R5.3: a session that prompts against a runtime that cannot prompt
        // would park on its first request with nothing able to answer. Fail
        // here, where the caller can still see why.
        if options
            .execution_policy
            .as_ref()
            .is_some_and(|policy| policy.interactive_approvals)
        {
            self.require_capability(capabilities::INTERACTIVE_APPROVALS)?;
        }
        let cwd = validate_cwd(&options.cwd)?;
        let requested_id = options.session_id.clone();
        let command = Command::SessionCreate {
            session_id: options.session_id,
            model: options.model,
            cwd,
            system_prompt: options.system_prompt,
            reasoning: options.reasoning,
            execution_policy: options.execution_policy,
            mcp_servers: options.mcp_servers,
            context: options.context,
        };
        let envelope = self
            .request_terminal(command, PendingKind::SessionCreate)
            .await?;
        let payload_id = match &envelope.event {
            Event::SessionCreated(created) => created.session_id.as_deref(),
            other => {
                return Err(Error::ProtocolViolation {
                    reason: format!(
                        "session_create terminated with `{}` instead of `session_created`",
                        other.event_type()
                    ),
                })
            }
        };
        let session_id = resolve_session_identity(
            requested_id.as_deref(),
            envelope.session_id.as_deref(),
            payload_id,
        )?;
        Ok(Session::new(self.clone(), session_id))
    }

    pub async fn resume_session(
        &self,
        session_id: impl Into<String>,
        options: ResumeOptions,
    ) -> Result<Session> {
        let session_id = session_id.into();
        let cwd = options.cwd.as_deref().map(validate_cwd).transpose()?;
        let command = Command::SessionResume {
            session_id: session_id.clone(),
            cwd,
            context: options.context,
        };
        let envelope = self
            .request_terminal(command, PendingKind::SessionResume)
            .await?;
        let payload_id = match &envelope.event {
            Event::SessionResumed(resumed) => resumed.session_id.as_deref(),
            other => {
                return Err(Error::ProtocolViolation {
                    reason: format!(
                        "session_resume terminated with `{}` instead of `session_resumed`",
                        other.event_type()
                    ),
                })
            }
        };
        let resumed_id = resolve_session_identity(
            Some(&session_id),
            envelope.session_id.as_deref(),
            payload_id,
        )?;
        Ok(Session::new(self.clone(), resumed_id))
    }

    /// Ask the runtime to exit and then close transport resources. This is the
    /// only graceful shutdown path; dropping all SDK handles uses `abort`.
    pub async fn shutdown(&self) -> Result<()> {
        let _guard = self.inner.shutdown_lock.lock().await;
        if self.inner.shutdown_complete.load(Ordering::Acquire) {
            return Ok(());
        }

        let envelope = self
            .request_terminal(Command::Shutdown, PendingKind::Shutdown)
            .await?;
        if !matches!(envelope.event, Event::CommandAck(_)) {
            return Err(Error::ProtocolViolation {
                reason: format!(
                    "shutdown terminated with `{}` instead of `command_ack`",
                    envelope.event.event_type()
                ),
            });
        }

        self.inner
            .shutdown_acknowledged
            .store(true, Ordering::Release);
        self.inner.fail_all(Error::ClientShutdown);
        let close_result = self
            .inner
            .transport
            .close()
            .await
            .map_err(|error| Error::Transport {
                message: error.message().to_owned(),
            });
        self.inner.stop_reader().await;
        if close_result.is_ok() {
            self.inner.shutdown_complete.store(true, Ordering::Release);
        }
        close_result
    }

    async fn request_terminal(&self, command: Command, kind: PendingKind) -> Result<EventEnvelope> {
        let mut receiver = self.inner.send_request(command, kind, None).await?;
        while let Some(item) = receiver.recv().await {
            let envelope = item?;
            if let Event::Error(error) = &envelope.event {
                return Err(runtime_error(error));
            }
            if kind.is_terminal(&envelope.event) {
                return Ok(envelope);
            }
        }
        Err(self.inner.failure_or_else(|| Error::ProtocolViolation {
            reason: "request event stream ended before its terminal event".into(),
        }))
    }

    async fn start_run(
        &self,
        session_id: String,
        prompt: String,
        options: RunOptions,
    ) -> Result<RunHandle> {
        let control = Arc::new(RunControl::default());
        let command = Command::RunStart {
            session_id,
            prompt,
            output_schema: options.output_schema,
            max_turns: options.max_turns,
            token_budget: options.token_budget,
            timeout_secs: options.timeout_secs,
        };
        let receiver = self
            .inner
            .send_request(command, PendingKind::Run, Some(Arc::clone(&control)))
            .await?;
        Ok(RunHandle {
            client: self.clone(),
            receiver,
            control,
            result: None,
            terminal_error: None,
        })
    }

    async fn run_control_command(&self, command: Command) -> Result<()> {
        let envelope = self
            .request_terminal(command, PendingKind::CommandAck)
            .await?;
        if matches!(envelope.event, Event::CommandAck(_)) {
            Ok(())
        } else {
            Err(Error::ProtocolViolation {
                reason: format!(
                    "control command terminated with `{}` instead of `command_ack`",
                    envelope.event.event_type()
                ),
            })
        }
    }
}

/// A loaded durable session. Dropping this value sends no command and does not
/// delete or close durable runtime state.
#[derive(Clone)]
pub struct Session {
    client: Client,
    id: Arc<str>,
    closed: Arc<AtomicBool>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Session {
    fn new(client: Client, id: String) -> Self {
        Self {
            client,
            id: Arc::from(id),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub async fn run(&self, prompt: impl Into<String>) -> Result<RunHandle> {
        self.run_with_options(prompt, RunOptions::default()).await
    }

    pub async fn run_with_options(
        &self,
        prompt: impl Into<String>,
        options: RunOptions,
    ) -> Result<RunHandle> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::SessionClosed {
                session_id: self.id.to_string(),
            });
        }
        self.client
            .start_run(self.id.to_string(), prompt.into(), options)
            .await
    }

    /// Unload the session from this runtime. The wire command does not delete
    /// durable session data, so it may subsequently be resumed.
    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let envelope = self
            .client
            .request_terminal(
                Command::SessionClose {
                    session_id: self.id.to_string(),
                },
                PendingKind::SessionClose,
            )
            .await?;
        let payload_id = match &envelope.event {
            Event::SessionClosed(closed) => closed.session_id.as_deref(),
            other => {
                return Err(Error::ProtocolViolation {
                    reason: format!(
                        "session_close terminated with `{}` instead of `session_closed`",
                        other.event_type()
                    ),
                })
            }
        };
        resolve_session_identity(Some(&self.id), envelope.session_id.as_deref(), payload_id)?;
        Ok(())
    }
}

/// One permission request the runtime is parked on.
///
/// Answer it. A request nobody answers does not fail and does not time out --
/// the run parks until some client responds (daemon-core R3.3), so dropping
/// these without answering is how a session hangs silently.
pub struct ApprovalRequest {
    inner: Arc<Inner>,
    requested: ApprovalRequested,
}

impl ApprovalRequest {
    /// The runtime's id for this request, which the answer is keyed on.
    pub fn id(&self) -> &str {
        &self.requested.request_id
    }

    /// Tool the run wants to use.
    pub fn tool(&self) -> &str {
        &self.requested.tool
    }

    /// Human-readable description of what is being asked.
    pub fn summary(&self) -> &str {
        &self.requested.summary
    }

    /// Registry row parked on this request, when the runtime knows it.
    pub fn work_id(&self) -> Option<&str> {
        self.requested.work_id.as_deref()
    }

    /// Whether `AlwaysAllow` is meaningful here; it is not always offered.
    pub fn always_allow_available(&self) -> bool {
        self.requested.always_allow_available
    }

    /// The whole wire payload, for fields this handle does not name.
    pub fn payload(&self) -> &ApprovalRequested {
        &self.requested
    }

    /// Answer it.
    ///
    /// Another attached client may have answered first; that is not a failure
    /// of yours, so it returns [`ApprovalOutcome::AlreadyResolved`] rather than
    /// an error (daemon-core R3.3).
    pub async fn respond(&self, decision: ApprovalDecision) -> Result<ApprovalOutcome> {
        let client = Client {
            inner: Arc::clone(&self.inner),
        };
        let result = client
            .request_terminal(
                Command::ApprovalRespond {
                    request_id: self.requested.request_id.clone(),
                    decision,
                },
                PendingKind::CommandAck,
            )
            .await;
        match result {
            Ok(_) => Ok(ApprovalOutcome::Recorded),
            // The runtime says `already_resolved` when someone else got there
            // first, or when the request never existed. Both mean the same
            // thing to a caller: stop waiting on it.
            Err(Error::Runtime { ref code, .. }) if code == "already_resolved" => {
                Ok(ApprovalOutcome::AlreadyResolved)
            }
            Err(error) => Err(error),
        }
    }
}

/// What became of an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// The runtime took this answer and the run continues.
    Recorded,
    /// Another client answered first, or the request had already lapsed.
    AlreadyResolved,
}

/// Permission requests the runtime raises, for the application to answer.
///
/// Draining this is not optional once a session enables approvals: an
/// unanswered request parks its run indefinitely.
pub struct ApprovalStream {
    requests: mpsc::Receiver<ApprovalRequest>,
}

impl ApprovalStream {
    /// Next request, or `None` once the connection is gone.
    pub async fn next_request(&mut self) -> Option<ApprovalRequest> {
        self.requests.recv().await
    }
}

/// Notices that the runtime's work registry moved.
///
/// Each carries the new revision; re-list to see what changed. Dropping it
/// stops delivery -- the runtime keeps its own subscription, but this client
/// stops holding the notices.
pub struct WorkWatcher {
    changes: mpsc::Receiver<WorkChanged>,
}

impl WorkWatcher {
    /// Next registry change, or `None` once the connection is gone.
    pub async fn next_change(&mut self) -> Option<WorkChanged> {
        self.changes.recv().await
    }
}

/// A hosted session this client is observing but does not own.
///
/// Dropping it detaches. The runtime keeps fanning events at an attachment
/// until told otherwise, so a handle that goes away without saying so leaves
/// the runtime talking to nobody; `Drop` sends the detach so the common case
/// cannot leak. Call [`AttachedSession::detach`] to observe the result.
pub struct AttachedSession {
    inner: Arc<Inner>,
    session_id: String,
    events: mpsc::Receiver<EventEnvelope>,
    detached: bool,
}

impl AttachedSession {
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Next event for this session, or `None` once detached.
    pub async fn next_event(&mut self) -> Option<EventEnvelope> {
        self.events.recv().await
    }

    /// Stop observing. The session itself is untouched and keeps running.
    pub async fn detach(mut self) -> Result<()> {
        self.detach_inner().await
    }

    async fn detach_inner(&mut self) -> Result<()> {
        if self.detached {
            return Ok(());
        }
        self.detached = true;
        self.inner.remove_attachment(&self.session_id);
        let client = Client {
            inner: Arc::clone(&self.inner),
        };
        let envelope = client
            .request_terminal(
                Command::SessionDetach {
                    session_id: self.session_id.clone(),
                },
                PendingKind::SessionDetach,
            )
            .await?;
        match envelope.event {
            Event::SessionDetached(_) => Ok(()),
            other => Err(Error::ProtocolViolation {
                reason: format!(
                    "session_detach terminated with `{}` instead of `session_detached`",
                    other.event_type()
                ),
            }),
        }
    }
}

impl Drop for AttachedSession {
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        self.detached = true;
        // Stop claiming events synchronously, so anything already in flight
        // stops being delivered the moment the handle is gone.
        self.inner.remove_attachment(&self.session_id);

        // Telling the runtime needs the wire, which needs an executor we may
        // not be on. Best-effort: if there is no runtime to spawn onto, the
        // local table is already cleared and the worst case is the runtime
        // fanning at an attachment this client now ignores.
        let inner = Arc::clone(&self.inner);
        let session_id = self.session_id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let client = Client { inner };
                let _ = client
                    .request_terminal(
                        Command::SessionDetach { session_id },
                        PendingKind::SessionDetach,
                    )
                    .await;
            });
        }
    }
}

/// Streaming handle for one active run.
///
/// It implements [`Stream`] with typed [`EventEnvelope`] items and also offers
/// [`RunHandle::next_event`] for callers that do not use stream extensions.
pub struct RunHandle {
    client: Client,
    receiver: mpsc::Receiver<Result<EventEnvelope>>,
    control: Arc<RunControl>,
    result: Option<RunResult>,
    terminal_error: Option<Error>,
}

impl RunHandle {
    pub fn known_run_id(&self) -> Option<String> {
        self.control.run_id()
    }

    pub async fn next_event(&mut self) -> Result<Option<EventEnvelope>> {
        match self.receiver.recv().await {
            Some(item) => self.record_item(item).map(Some),
            None => {
                if let Some(error) = &self.terminal_error {
                    Err(error.clone())
                } else if self.result.is_some() {
                    Ok(None)
                } else {
                    Err(self
                        .client
                        .inner
                        .failure_or_else(|| Error::ProtocolViolation {
                            reason: "run event stream ended without `run_completed`".into(),
                        }))
                }
            }
        }
    }

    /// Consume the handle and buffer events until `run_completed`.
    pub async fn final_result(mut self) -> Result<RunResult> {
        if let Some(result) = self.result.take() {
            return Ok(result);
        }
        if let Some(error) = self.terminal_error.take() {
            return Err(error);
        }
        loop {
            match self.next_event().await? {
                Some(_) if self.result.is_some() => {
                    return Ok(self.result.take().expect("result was just checked"))
                }
                Some(_) => {}
                None => {
                    if let Some(result) = self.result.take() {
                        return Ok(result);
                    }
                    return Err(Error::ProtocolViolation {
                        reason: "run event stream ended without `run_completed`".into(),
                    });
                }
            }
        }
    }

    pub async fn cancel(&self) -> Result<()> {
        let run_id = self.control.wait_for_run_id().await?;
        self.client
            .run_control_command(Command::RunCancel { run_id })
            .await
    }

    pub async fn steer(&self, instruction: impl Into<String>) -> Result<()> {
        let run_id = self.control.wait_for_run_id().await?;
        self.client
            .run_control_command(Command::RunSteer {
                run_id,
                instruction: instruction.into(),
            })
            .await
    }

    fn record_item(&mut self, item: Result<EventEnvelope>) -> Result<EventEnvelope> {
        match item {
            Ok(envelope) => {
                match &envelope.event {
                    Event::RunCompleted(completed) => {
                        self.result = Some(completed.clone().into());
                    }
                    Event::Error(error) => {
                        self.terminal_error = Some(runtime_error(error));
                    }
                    _ => {}
                }
                Ok(envelope)
            }
            Err(error) => {
                self.terminal_error = Some(error.clone());
                Err(error)
            }
        }
    }
}

impl Stream for RunHandle {
    type Item = Result<EventEnvelope>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.receiver).poll_recv(cx) {
            Poll::Ready(Some(item)) => Poll::Ready(Some(this.record_item(item))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingKind {
    Handshake,
    SessionCreate,
    SessionResume,
    SessionClose,
    SessionList,
    SessionAttach,
    SessionDetach,
    WorkList,
    WorkAction,
    Run,
    CommandAck,
    Shutdown,
}

impl PendingKind {
    fn is_terminal(self, event: &Event) -> bool {
        matches!(event, Event::Error(_))
            || matches!(
                (self, event),
                (Self::Handshake, Event::RuntimeReady(_))
                    | (Self::SessionCreate, Event::SessionCreated(_))
                    | (Self::SessionResume, Event::SessionResumed(_))
                    | (Self::SessionClose, Event::SessionClosed(_))
                    | (Self::SessionList, Event::SessionListed(_))
                    | (Self::SessionAttach, Event::SessionAttached(_))
                    | (Self::SessionDetach, Event::SessionDetached(_))
                    | (Self::WorkList, Event::WorkListed(_))
                    | (Self::WorkAction, Event::WorkActioned(_))
                    | (Self::Run, Event::RunCompleted(_))
                    | (Self::CommandAck, Event::CommandAck(_))
                    | (Self::Shutdown, Event::CommandAck(_))
            )
    }
}

struct PendingRequest {
    sender: mpsc::Sender<Result<EventEnvelope>>,
    kind: PendingKind,
    run_control: Option<Arc<RunControl>>,
}

#[derive(Clone, Debug, Default)]
struct RunControlState {
    run_id: Option<String>,
    terminal: bool,
    /// Why the run ended, when it ended because the connection failed
    /// rather than with its own terminal event.
    failure: Option<Error>,
}

struct RunControl {
    state: watch::Sender<RunControlState>,
}

impl Default for RunControl {
    fn default() -> Self {
        let (state, _) = watch::channel(RunControlState::default());
        Self { state }
    }
}

impl RunControl {
    fn run_id(&self) -> Option<String> {
        self.state.borrow().run_id.clone()
    }

    fn set_run_id(&self, run_id: &str) -> Result<()> {
        let current = self.state.borrow().run_id.clone();
        if let Some(current) = current.as_deref() {
            if current != run_id {
                return Err(Error::ProtocolViolation {
                    reason: format!(
                        "runtime changed run_id from `{current}` to `{run_id}` during a run"
                    ),
                });
            }
        } else {
            self.state.send_modify(|state| {
                state.run_id = Some(run_id.to_owned());
            });
        }
        Ok(())
    }

    fn finish(&self) {
        self.state.send_modify(|state| state.terminal = true);
    }

    /// End the run because the connection failed with `error`.
    fn fail(&self, error: Error) {
        self.state.send_modify(|state| {
            state.terminal = true;
            state.failure = Some(error);
        });
    }

    fn is_terminal(&self) -> bool {
        self.state.borrow().terminal
    }

    async fn wait_for_run_id(&self) -> Result<String> {
        let mut state = self.state.subscribe();
        loop {
            let snapshot = state.borrow().clone();
            if let Some(error) = snapshot.failure {
                return Err(error);
            }
            if snapshot.terminal {
                return Err(Error::RunFinished {
                    run_id: snapshot.run_id.unwrap_or_else(|| "<not assigned>".into()),
                });
            }
            if let Some(run_id) = snapshot.run_id {
                return Ok(run_id);
            }
            if state.changed().await.is_err() {
                return Err(Error::RunFinished {
                    run_id: "<not assigned>".into(),
                });
            }
        }
    }
}

struct Inner {
    transport: Arc<dyn Transport>,
    pending: Mutex<HashMap<String, PendingRequest>>,
    /// Sinks for events that belong to an attached session rather than to a
    /// request this client sent, keyed by session id.
    ///
    /// Every other event is answered traffic: the runtime replies with the
    /// `request_id` we gave it, and `route` matches it back. Attachment breaks
    /// that, by design -- the point is to observe a session someone else is
    /// driving -- so the runtime fans those events out carrying the
    /// *originating* client's `request_id`, or an empty one for events it
    /// raised itself (`agent_runtime.rs`, `emit`). Neither is a request of
    /// ours, so without this table the first fanned-out event would be an
    /// unknown-`request_id` protocol violation and would fail the connection.
    attachments: Mutex<HashMap<String, mpsc::Sender<EventEnvelope>>>,
    /// Sink for `WorkChanged`, which belongs to no session and answers no
    /// request: it reports that the runtime's registry moved, and reaches only
    /// connections that asked for the work list.
    work_watchers: Mutex<Vec<mpsc::Sender<WorkChanged>>>,
    /// Sink for approval requests the runtime raised. Like session events they
    /// name a session and answer no request of ours; unlike them they are not
    /// optional to read, because a request nobody answers parks its run.
    ///
    /// It exists from connect rather than from the first `approvals()` call.
    /// The runtime can raise a request the instant a session starts prompting,
    /// which may be before the application has asked for the stream; an
    /// approval with nowhere to go used to fall through to the unknown-request
    /// rule and fail the whole connection. Buffering from the start means the
    /// race cannot happen, and a caller that never reads simply leaves
    /// requests queued -- visibly parked, rather than a dead connection.
    approval_sender: mpsc::Sender<ApprovalRequest>,
    approval_receiver: AsyncMutex<Option<mpsc::Receiver<ApprovalRequest>>>,
    next_request_id: AtomicU64,
    terminal_error: Mutex<Option<Error>>,
    runtime_info: Mutex<Option<RuntimeInfo>>,
    negotiated_protocol: AtomicU32,
    shutdown_acknowledged: AtomicBool,
    shutdown_complete: AtomicBool,
    shutdown_lock: AsyncMutex<()>,
    send_lock: AsyncMutex<()>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
}

impl Inner {
    fn new(transport: Arc<dyn Transport>) -> Self {
        let (approval_sender, approval_receiver) = mpsc::channel(APPROVAL_BUFFER);
        Self {
            transport,
            pending: Mutex::new(HashMap::new()),
            attachments: Mutex::new(HashMap::new()),
            work_watchers: Mutex::new(Vec::new()),
            approval_sender,
            approval_receiver: AsyncMutex::new(Some(approval_receiver)),
            next_request_id: AtomicU64::new(1),
            terminal_error: Mutex::new(None),
            runtime_info: Mutex::new(None),
            negotiated_protocol: AtomicU32::new(0),
            shutdown_acknowledged: AtomicBool::new(false),
            shutdown_complete: AtomicBool::new(false),
            shutdown_lock: AsyncMutex::new(()),
            send_lock: AsyncMutex::new(()),
            reader_task: Mutex::new(None),
        }
    }

    fn start_reader(this: &Arc<Self>) {
        let weak = Arc::downgrade(this);
        let transport = Arc::clone(&this.transport);
        let task = tokio::spawn(async move { reader_loop(transport, weak).await });
        *lock(&this.reader_task) = Some(task);
    }

    async fn stop_reader(&self) {
        let task = lock(&self.reader_task).take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    async fn send_request(
        &self,
        command: Command,
        kind: PendingKind,
        run_control: Option<Arc<RunControl>>,
    ) -> Result<mpsc::Receiver<Result<EventEnvelope>>> {
        let _send_guard = self.send_lock.lock().await;
        if let Some(error) = lock(&self.terminal_error).clone() {
            return Err(error);
        }
        if self.shutdown_acknowledged.load(Ordering::Acquire) {
            return Err(Error::ClientShutdown);
        }

        let request_id = format!(
            "sdk-{}",
            self.next_request_id.fetch_add(1, Ordering::Relaxed)
        );
        let request = RequestEnvelope::new(request_id.clone(), command);
        let line = serde_json::to_string(&request).map_err(|_| Error::Serialization)?;
        let (sender, receiver) = mpsc::channel(EVENT_BUFFER_CAPACITY);

        lock(&self.pending).insert(
            request_id.clone(),
            PendingRequest {
                sender,
                kind,
                run_control,
            },
        );

        if let Err(error) = self.transport.send_line(line).await {
            lock(&self.pending).remove(&request_id);
            let error = Error::Transport {
                message: error.message().to_owned(),
            };
            self.fail_all(error.clone());
            return Err(error);
        }
        Ok(receiver)
    }

    /// Deliver a fanned-out event to its attachment, if we have one.
    ///
    /// Returns whether the event was claimed. A full or closed channel still
    /// counts as claimed: the event is for that attachment either way, and a
    /// slow or dropped reader must not escalate into a connection-wide
    /// protocol violation.
    fn route_to_attachment(&self, envelope: &EventEnvelope) -> bool {
        let Some(session_id) = envelope.session_id.as_deref() else {
            return false;
        };
        let attachments = lock(&self.attachments);
        let Some(sender) = attachments.get(session_id) else {
            return false;
        };
        let _ = sender.try_send(envelope.clone());
        true
    }

    /// Fan a registry change out to everyone watching, dropping senders whose
    /// receiver is gone. A watcher that fell behind loses the notice rather
    /// than stalling the connection; the revision is a hint to re-list, not
    /// state the caller must not miss.
    fn route_to_work_watchers(&self, changed: &WorkChanged) {
        let mut watchers = lock(&self.work_watchers);
        watchers.retain(|sender| !sender.is_closed());
        for sender in watchers.iter() {
            let _ = sender.try_send(*changed);
        }
    }

    /// Hand an approval request to the application, if it is listening.
    ///
    /// Returns whether it was claimed. Unclaimed falls through to the ordinary
    /// routing rules: an application that never called `approvals()` cannot
    /// have created a session that prompts, so the request is not ours.
    fn route_to_approvals(self: &Arc<Self>, requested: &ApprovalRequested) {
        let _ = self.approval_sender.try_send(ApprovalRequest {
            inner: Arc::clone(self),
            requested: requested.clone(),
        });
    }

    fn register_work_watcher(&self, sender: mpsc::Sender<WorkChanged>) {
        lock(&self.work_watchers).push(sender);
    }

    fn register_attachment(&self, session_id: &str, sender: mpsc::Sender<EventEnvelope>) {
        lock(&self.attachments).insert(session_id.to_owned(), sender);
    }

    fn remove_attachment(&self, session_id: &str) {
        lock(&self.attachments).remove(session_id);
    }

    fn route(self: &Arc<Self>, envelope: EventEnvelope) -> Result<()> {
        let request_id = envelope.request_id.clone();
        let mut pending = lock(&self.pending);
        let Some(request) = pending.get(&request_id) else {
            // Not answering a request of ours. If it names a session we are
            // attached to, it is fan-out and belongs to that attachment;
            // otherwise the runtime is talking about something we never asked
            // for, which stays a protocol violation.
            drop(pending);
            if let Event::WorkChanged(changed) = &envelope.event {
                self.route_to_work_watchers(changed);
                return Ok(());
            }
            if let Event::ApprovalRequested(requested) = &envelope.event {
                Inner::route_to_approvals(self, requested);
                return Ok(());
            }
            if self.route_to_attachment(&envelope) {
                return Ok(());
            }
            return Err(Error::ProtocolViolation {
                reason: format!(
                    "runtime emitted an event for unknown request_id `{}`",
                    envelope.request_id
                ),
            });
        };

        if let Some(control) = &request.run_control {
            if control.is_terminal() {
                return Err(Error::ProtocolViolation {
                    reason: "runtime emitted a run event after its terminal event".into(),
                });
            }
            if let Event::RunStarted(started) = &envelope.event {
                let run_id = match (envelope.run_id.as_deref(), started.run_id.as_deref()) {
                    (Some(envelope_id), Some(payload_id)) if envelope_id != payload_id => {
                        return Err(Error::ProtocolViolation {
                            reason: format!(
                                "run_started payload run_id `{payload_id}` disagrees with envelope run_id `{envelope_id}`"
                            ),
                        })
                    }
                    (Some(run_id), _) | (_, Some(run_id)) => run_id,
                    (None, None) => {
                        return Err(Error::ProtocolViolation {
                            reason: "run_started omitted run_id from both envelope and event".into(),
                        })
                    }
                };
                control.set_run_id(run_id)?;
            } else if let (Some(known), Some(received)) =
                (control.run_id(), envelope.run_id.as_deref())
            {
                if known != received {
                    return Err(Error::ProtocolViolation {
                        reason: format!(
                            "run event used run_id `{received}` after run `{known}` started"
                        ),
                    });
                }
            }
        }

        let terminal = request.kind.is_terminal(&envelope.event);
        let shutdown_ack =
            request.kind == PendingKind::Shutdown && matches!(envelope.event, Event::CommandAck(_));
        let run_control = request.run_control.clone();
        // A dropped receiver is not a protocol failure. Keep routing/discarding
        // until the terminal event so later output does not look unsolicited.
        match request.sender.try_send(Ok(envelope)) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(_)) => return Err(Error::EventBufferOverflow),
        }
        if terminal {
            if let Some(control) = run_control {
                control.finish();
            }
            pending.remove(&request_id);
        }
        drop(pending);

        if shutdown_ack {
            self.shutdown_acknowledged.store(true, Ordering::Release);
        }
        Ok(())
    }

    fn fail_all(&self, error: Error) {
        let mut terminal_error = lock(&self.terminal_error);
        if terminal_error.is_some() {
            return;
        }
        *terminal_error = Some(error.clone());
        drop(terminal_error);

        let pending = std::mem::take(&mut *lock(&self.pending));
        for (_, request) in pending {
            if let Some(control) = request.run_control {
                control.fail(error.clone());
            }
            let _ = request.sender.try_send(Err(error.clone()));
        }
    }

    fn failure_or_else(&self, fallback: impl FnOnce() -> Error) -> Error {
        lock(&self.terminal_error).clone().unwrap_or_else(fallback)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if !self.shutdown_complete.load(Ordering::Acquire) {
            self.transport.abort();
        }
        if let Some(task) = lock(&self.reader_task).take() {
            task.abort();
        }
    }
}

async fn reader_loop(transport: Arc<dyn Transport>, inner: Weak<Inner>) {
    let mut last_sequence = None;
    let mut runtime_id: Option<String> = None;
    loop {
        let line = match transport.receive_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                if let Some(inner) = inner.upgrade() {
                    if inner.shutdown_acknowledged.load(Ordering::Acquire) {
                        inner.fail_all(Error::ClientShutdown);
                    } else {
                        inner.fail_all(Error::RuntimeExited {
                            diagnostics: transport.diagnostics(),
                        });
                    }
                }
                return;
            }
            Err(error) => {
                if let Some(inner) = inner.upgrade() {
                    inner.fail_all(Error::Transport {
                        message: error.message().to_owned(),
                    });
                }
                return;
            }
        };

        let envelope: EventEnvelope = match serde_json::from_str(&line) {
            Ok(envelope) => envelope,
            Err(error) => {
                if let Some(inner) = inner.upgrade() {
                    inner.fail_all(Error::MalformedProtocol {
                        reason: error.to_string(),
                    });
                }
                transport.abort();
                return;
            }
        };

        let Some(inner) = inner.upgrade() else {
            transport.abort();
            return;
        };
        if let Err(error) =
            validate_envelope(&inner, &envelope, &mut last_sequence, &mut runtime_id)
                .and_then(|()| inner.route(envelope))
        {
            inner.fail_all(error);
            transport.abort();
            return;
        }
    }
}

fn validate_envelope(
    inner: &Inner,
    envelope: &EventEnvelope,
    last_sequence: &mut Option<u64>,
    runtime_id: &mut Option<String>,
) -> Result<()> {
    let negotiated = inner.negotiated_protocol.load(Ordering::Acquire);
    let version_allowed = if negotiated == 0 {
        SUPPORTED_PROTOCOL_VERSIONS.contains(&envelope.protocol_version)
    } else {
        envelope.protocol_version == negotiated
    };
    if !version_allowed {
        return Err(Error::ProtocolVersionMismatch {
            sdk_supported: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            runtime_supported: vec![envelope.protocol_version],
        });
    }
    // An empty `request_id` is the runtime emitting on its own behalf rather
    // than answering anyone -- approvals it raised, work it noticed. Session
    // events name their session and `route` decides whether we are attached;
    // `WorkChanged` names nothing at all, because it is about the runtime's
    // whole registry rather than one session, and only reaches connections
    // that asked for the work list. Everything else unaddressed stays a
    // violation.
    let unaddressed_but_expected =
        envelope.session_id.is_some() || matches!(envelope.event, Event::WorkChanged(_));
    if envelope.request_id.is_empty() && !unaddressed_but_expected {
        return Err(Error::ProtocolViolation {
            reason: "event request_id is empty".into(),
        });
    }
    if let Some(previous) = *last_sequence {
        if envelope.sequence <= previous {
            return Err(Error::ProtocolViolation {
                reason: format!(
                    "event sequence {} is not greater than previous sequence {previous}",
                    envelope.sequence
                ),
            });
        }
    }
    *last_sequence = Some(envelope.sequence);

    OffsetDateTime::parse(&envelope.timestamp, &Rfc3339).map_err(|_| Error::MalformedProtocol {
        reason: "event timestamp is not RFC3339".into(),
    })?;

    if envelope.runtime_id.is_empty() {
        return Err(Error::ProtocolViolation {
            reason: "event runtime_id is empty".into(),
        });
    }
    match runtime_id {
        Some(expected) if expected != &envelope.runtime_id => {
            return Err(Error::ProtocolViolation {
                reason: format!(
                    "runtime_id changed from `{expected}` to `{}`",
                    envelope.runtime_id
                ),
            })
        }
        Some(_) => {}
        None => *runtime_id = Some(envelope.runtime_id.clone()),
    }
    Ok(())
}

fn negotiate_compatibility(ready: &RuntimeReady, options: &ClientOptions) -> Result<RuntimeInfo> {
    let protocol_version = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|version| ready.supported_protocol_versions.contains(version))
        .ok_or_else(|| Error::ProtocolVersionMismatch {
            sdk_supported: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            runtime_supported: ready.supported_protocol_versions.clone(),
        })?;

    let runtime_version =
        Version::parse(&ready.runtime_version).map_err(|_| Error::InvalidRuntimeVersion {
            version: ready.runtime_version.clone(),
        })?;
    if !options
        .runtime_version_requirement
        .matches(&runtime_version)
    {
        return Err(Error::RuntimeVersionIncompatible {
            version: runtime_version.to_string(),
            requirement: options.runtime_version_requirement.to_string(),
        });
    }

    let capabilities: HashSet<_> = ready.capabilities.iter().map(String::as_str).collect();
    let missing: Vec<_> = options
        .required_capabilities
        .iter()
        .filter(|capability| !capabilities.contains(capability.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(Error::MissingCapabilities { missing });
    }

    Ok(RuntimeInfo {
        runtime_version,
        protocol_version,
        capabilities: ready.capabilities.clone(),
    })
}

fn validate_cwd(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path).map_err(|error| Error::InvalidWorkingDirectory {
        path: path.to_owned(),
        reason: error.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(Error::InvalidWorkingDirectory {
            path: path.to_owned(),
            reason: "path is not a directory".into(),
        });
    }
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::InvalidWorkingDirectory {
            path: path.to_owned(),
            reason: "path is not valid UTF-8".into(),
        })
}

fn resolve_session_identity(
    requested: Option<&str>,
    envelope_id: Option<&str>,
    payload_id: Option<&str>,
) -> Result<String> {
    if let (Some(envelope_id), Some(payload_id)) = (envelope_id, payload_id) {
        if envelope_id != payload_id {
            return Err(Error::ProtocolViolation {
                reason: format!(
                    "event payload session_id `{payload_id}` disagrees with envelope session_id `{envelope_id}`"
                ),
            });
        }
    }
    let received = envelope_id
        .or(payload_id)
        .ok_or_else(|| Error::ProtocolViolation {
            reason: "session event omitted session_id from both envelope and event".into(),
        })?;
    if received.is_empty() {
        return Err(Error::ProtocolViolation {
            reason: "runtime returned an empty session_id".into(),
        });
    }
    if let Some(requested) = requested {
        if requested != received {
            return Err(Error::ProtocolViolation {
                reason: format!(
                    "runtime returned session `{received}` for requested session `{requested}`"
                ),
            });
        }
    }
    Ok(received.to_owned())
}

fn runtime_error(error: &o4_agent_protocol::ErrorEvent) -> Error {
    Error::Runtime {
        code: error.code.clone(),
        message: error.message.clone(),
        retryable: error.retryable,
    }
}

fn deduplicate(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
