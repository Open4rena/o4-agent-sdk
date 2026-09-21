//! Version 1 JSONL wire types shared by the o4 runtime and SDK.
//!
//! The protocol is additive: structs intentionally do not use
//! `deny_unknown_fields`, so a v1 peer may add fields without breaking older
//! clients. Unknown event tags are preserved as [`Event::Unknown`]. Peers use
//! the initialize/runtime-ready capability lists before relying on optional
//! events or behavior.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The JSONL wire protocol implemented by this crate.
pub const PROTOCOL_VERSION: u32 = 1;
/// Protocol versions this SDK can negotiate, in preference order.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[PROTOCOL_VERSION];
/// SDK capabilities advertised during initialization.
pub const SDK_CAPABILITIES: &[&str] =
    &["event.unknown", "run.cancel", "run.steer", "session.resume"];
/// Placeholder runtime floor, deliberately separate from wire compatibility.
///
/// Until the runtime publishes its post-version-bump floor, this accepts every
/// valid stable semver. Applications can configure a stricter requirement in
/// the SDK without changing protocol negotiation.
pub const DEFAULT_RUNTIME_VERSION_REQUIREMENT: &str = ">=0.0.0";

/// One command, encoded as exactly one JSON object followed by `\n`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub command: Command,
}

impl RequestEnvelope {
    pub fn new(request_id: impl Into<String>, command: Command) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            command,
        }
    }
}

/// One runtime event, encoded as exactly one JSON object followed by `\n`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub sequence: u64,
    /// RFC3339 timestamp supplied by the runtime.
    pub timestamp: String,
    pub runtime_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
    pub event: Event,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Initialize {
        client_name: String,
        client_version: String,
        supported_protocol_versions: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
    },
    SessionCreate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        model: String,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<Reasoning>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_policy: Option<ExecutionPolicy>,
        #[serde(default)]
        mcp_servers: Vec<McpServerConfig>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<Context>,
    },
    SessionResume {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<Context>,
    },
    RunStart {
        session_id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_schema: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_turns: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_budget: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
    },
    RunCancel {
        run_id: String,
    },
    RunSteer {
        run_id: String,
        instruction: String,
    },
    SessionClose {
        session_id: String,
    },
    Shutdown,

    // --- Daemon additions (spec daemon-core §6). Additive: a v1 peer that
    // never sends them is unaffected; a runtime advertises support through
    // `RuntimeReady.capabilities`.
    /// List every session the runtime currently hosts.
    SessionList,
    /// Subscribe this connection to a hosted session's events.
    SessionAttach {
        session_id: String,
    },
    /// Unsubscribe this connection from a hosted session's events.
    SessionDetach {
        session_id: String,
    },
    /// Snapshot the runtime's work registry.
    WorkList,
    /// Act on one work-registry row.
    WorkAction {
        work_id: String,
        action: WorkActionKind,
    },
    /// Answer a pending permission request fanned out as `ApprovalRequested`.
    ApprovalRespond {
        request_id: String,
        decision: ApprovalDecision,
    },
    /// Answer a pending user question fanned out as `QuestionAsked`.
    QuestionAnswer {
        request_id: String,
        answer: String,
    },
    /// Quiesce a session's active run ahead of a handoff seal.
    RunDrain {
        session_id: String,
        #[serde(default)]
        now: bool,
    },
    /// Report a committed ownership change for a hosted session.
    SessionOwnership {
        session_id: String,
        owner: OwnerWire,
        owner_generation: i64,
    },
}

/// What a `WorkAction` does to a registry row (spec daemon-core R4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkActionKind {
    Stop,
    Message { text: String },
    Foreground,
    Rename { name: String },
    Clear,
}

/// Decision for a fanned-out permission request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
    /// Approve and remember for the rest of the session where the runtime
    /// allows it (`ApprovalRequested.always_allow_available`).
    AlwaysAllow,
}

/// Who owns a hosted session. Mirrors the handoff transfer protocol's
/// `Owner` so a client can correlate to a transfer record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerWire {
    Local,
    Platform,
    Undecided,
}

/// Requested model reasoning effort.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    Minimal,
    Low,
    Medium,
    High,
}

/// Tool and sandbox permissions sent to the runtime.
///
/// Empty `allowed_tools` and `allowed_domains` mean no explicit grants. The
/// runtime remains responsible for applying its own secure baseline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub denied_tools: Vec<String>,
    pub sandbox_mode: SandboxMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_config: Option<Value>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Park side-effecting tool calls on an `ApprovalRequested` event and
    /// wait for an attached client's `ApprovalRespond` (spec daemon-core
    /// R3.3). Off by default: an SDK caller that cannot answer a prompt keeps
    /// the static allow/deny behavior. Requires the `interactive_approvals`
    /// capability.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interactive_approvals: bool,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            sandbox_mode: SandboxMode::ReadOnly,
            sandbox_config: None,
            allowed_domains: Vec::new(),
            interactive_approvals: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    Disabled,
    #[default]
    ReadOnly,
    WorkspaceWrite,
}

/// Configuration for one Model Context Protocol server.
///
/// Serialization includes the values needed by the runtime. The custom
/// [`Debug`] implementation deliberately omits environment values, header
/// values, URLs, and command arguments because they may contain credentials.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpServerConfig {
    Stdio {
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    Sse {
        name: String,
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// MCP Streamable HTTP transport (protocol revision 2025-03-26+).
    ///
    /// Secrets are never carried inline: `bearer_token_env_var` and
    /// `env_http_headers` name environment variables that the runtime reads
    /// from its own process at connect time.
    StreamableHttp {
        name: String,
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token_env_var: Option<String>,
        /// Header name -> environment variable name.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env_http_headers: BTreeMap<String, String>,
    },
}

impl fmt::Debug for McpServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio {
                name,
                command,
                args,
                env,
                cwd,
            } => f
                .debug_struct("McpServerConfig::Stdio")
                .field("name", name)
                .field("command", command)
                .field("args", &format_args!("<{} redacted>", args.len()))
                .field("env_keys", &env.keys().collect::<Vec<_>>())
                .field("cwd", cwd)
                .finish(),
            Self::Sse {
                name,
                url: _,
                headers,
            } => f
                .debug_struct("McpServerConfig::Sse")
                .field("name", name)
                .field("url", &"<redacted>")
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .finish(),
            Self::StreamableHttp {
                name,
                url: _,
                headers,
                bearer_token_env_var,
                env_http_headers,
            } => f
                .debug_struct("McpServerConfig::StreamableHttp")
                .field("name", name)
                .field("url", &"<redacted>")
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .field("bearer_token_env_var", bearer_token_env_var)
                .field("env_http_headers", env_http_headers)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeReady {
    pub runtime_version: String,
    pub supported_protocol_versions: Vec<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Present when the runtime is daemon-capable (spec daemon-core R6.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// Whether `[daemon] enabled` is on for this runtime's data dir.
    pub enabled: bool,
}

/// Capability strings a daemon-capable runtime advertises in
/// `RuntimeReady.capabilities` (spec daemon-core §6).
pub mod capabilities {
    pub const SESSION_LIST: &str = "session_list";
    pub const SESSION_ATTACH: &str = "session_attach";
    pub const WORK_REGISTRY: &str = "work_registry";
    pub const WORK_ACTION: &str = "work_action";
    pub const INTERACTIVE_APPROVALS: &str = "interactive_approvals";
    pub const RUN_DRAIN: &str = "run_drain";
    pub const SESSION_OWNERSHIP: &str = "session_ownership";
}

/// One hosted session as reported by `SessionList` (spec daemon-core R2.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    /// The UUID the handoff transfer protocol keys on, when the session has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_uuid: Option<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: String,
    pub last_activity_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run: Option<String>,
    #[serde(default)]
    pub attached_clients: u32,
    pub owner: OwnerWire,
    #[serde(default)]
    pub owner_generation: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListed {
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
}

/// Registry row owner on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkOwnerWire {
    Session { session_id: String },
    Process,
}

/// A pending input request attached to a registry row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingWire {
    pub request_id: String,
    /// `approval` or `question`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// One work-registry row (spec daemon-core R4.2). Every field a dashboard
/// needs; durations are elapsed milliseconds at snapshot time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemWire {
    pub id: String,
    pub name: String,
    /// Registry `WorkKind` in snake_case (`main_agent`, `sub_agent`,
    /// `bash_task`, …).
    pub kind: String,
    /// Registry `WorkStatus` in snake_case.
    pub status: String,
    pub owner: WorkOwnerWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingWire>,
    /// Terminal detail (error text, exit summary) when the row is finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled_secs: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkCountsWire {
    #[serde(default)]
    pub needs_input: u32,
    #[serde(default)]
    pub active: u32,
    #[serde(default)]
    pub finished: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkListed {
    pub revision: u64,
    #[serde(default)]
    pub counts: WorkCountsWire,
    #[serde(default)]
    pub items: Vec<WorkItemWire>,
    /// True when the runtime cut the list at its row cap.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkActioned {
    pub work_id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkChanged {
    pub revision: u64,
}

/// A permission request fanned out to every attached client (spec
/// daemon-core R3.3). Answer with `Command::ApprovalRespond`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequested {
    pub request_id: String,
    /// Registry row that is parked on this request, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    pub tool: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default)]
    pub always_allow_available: bool,
}

/// A user question fanned out to every attached client. Answer with
/// `Command::QuestionAnswer`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAsked {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
}

/// Reply to `RunDrain` once the post-drain snapshot is committed (spec
/// daemon-core R5.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDrained {
    pub journal_sequence: u64,
    pub journal_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOwnershipReported {
    pub owner: OwnerWire,
    pub owner_generation: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// May duplicate the envelope field for runtimes that provide it in the
    /// payload. The envelope `session_id` remains authoritative metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStarted {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStarted {
    /// May duplicate the envelope field for runtimes that provide it in the
    /// payload. The envelope `run_id` remains authoritative metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStarted {
    pub turn: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageEvent {
    /// A complete message snapshot at this point in the stream.
    pub message: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolEvent {
    /// A complete tool-call snapshot at this point in the stream.
    pub tool_call: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarningEvent {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouterDiagnostic {
    pub diagnostic: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryEvent {
    pub attempt: u32,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompacted {
    pub before_tokens: u64,
    pub after_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimit {
    pub resource: String,
    pub limit: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteeringInjected {
    pub instruction: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Cancelled,
    Failed,
    TimedOut,
    ResourceLimited,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunCompleted {
    pub status: RunStatus,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub tool_calls: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// A typed runtime event.
///
/// Unknown tags are retained rather than rejected. Applications should only
/// depend on optional event kinds after checking negotiated capabilities.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    RuntimeReady(RuntimeReady),
    SessionCreated(SessionEvent),
    SessionResumed(SessionEvent),
    SessionClosed(SessionEvent),
    RunStarted(RunStarted),
    AgentStarted(AgentStarted),
    TurnStarted(TurnStarted),
    MessageStarted(MessageEvent),
    MessageUpdated(MessageEvent),
    MessageCompleted(MessageEvent),
    ToolStarted(ToolEvent),
    ToolUpdated(ToolEvent),
    ToolCompleted(ToolEvent),
    Warning(WarningEvent),
    RouterDiagnostic(RouterDiagnostic),
    Retry(RetryEvent),
    ContextCompacted(ContextCompacted),
    ResourceLimit(ResourceLimit),
    SteeringInjected(SteeringInjected),
    RunCompleted(RunCompleted),
    CommandAck(CommandAck),
    Error(ErrorEvent),
    // --- Daemon additions (spec daemon-core §6).
    SessionListed(SessionListed),
    SessionAttached(SessionEvent),
    SessionDetached(SessionEvent),
    WorkListed(WorkListed),
    WorkActioned(WorkActioned),
    WorkChanged(WorkChanged),
    ApprovalRequested(ApprovalRequested),
    QuestionAsked(QuestionAsked),
    RunDrained(RunDrained),
    SessionOwnershipReported(SessionOwnershipReported),
    Unknown {
        event_type: String,
        fields: Map<String, Value>,
    },
}

impl Event {
    pub fn event_type(&self) -> &str {
        match self {
            Self::RuntimeReady(_) => "runtime_ready",
            Self::SessionCreated(_) => "session_created",
            Self::SessionResumed(_) => "session_resumed",
            Self::SessionClosed(_) => "session_closed",
            Self::RunStarted(_) => "run_started",
            Self::AgentStarted(_) => "agent_started",
            Self::TurnStarted(_) => "turn_started",
            Self::MessageStarted(_) => "message_started",
            Self::MessageUpdated(_) => "message_updated",
            Self::MessageCompleted(_) => "message_completed",
            Self::ToolStarted(_) => "tool_started",
            Self::ToolUpdated(_) => "tool_updated",
            Self::ToolCompleted(_) => "tool_completed",
            Self::Warning(_) => "warning",
            Self::RouterDiagnostic(_) => "router_diagnostic",
            Self::Retry(_) => "retry",
            Self::ContextCompacted(_) => "context_compacted",
            Self::ResourceLimit(_) => "resource_limit",
            Self::SteeringInjected(_) => "steering_injected",
            Self::RunCompleted(_) => "run_completed",
            Self::CommandAck(_) => "command_ack",
            Self::Error(_) => "error",
            Self::SessionListed(_) => "session_listed",
            Self::SessionAttached(_) => "session_attached",
            Self::SessionDetached(_) => "session_detached",
            Self::WorkListed(_) => "work_listed",
            Self::WorkActioned(_) => "work_actioned",
            Self::WorkChanged(_) => "work_changed",
            Self::ApprovalRequested(_) => "approval_requested",
            Self::QuestionAsked(_) => "question_asked",
            Self::RunDrained(_) => "run_drained",
            Self::SessionOwnershipReported(_) => "session_ownership_reported",
            Self::Unknown { event_type, .. } => event_type,
        }
    }
}

fn serialize_event<S, T>(event_type: &str, payload: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    let mut fields = match serde_json::to_value(payload).map_err(serde::ser::Error::custom)? {
        Value::Object(fields) => fields,
        _ => return Err(serde::ser::Error::custom("event payload was not an object")),
    };
    fields.insert("type".to_owned(), Value::String(event_type.to_owned()));
    fields.serialize(serializer)
}

impl Serialize for Event {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::RuntimeReady(v) => serialize_event("runtime_ready", v, serializer),
            Self::SessionCreated(v) => serialize_event("session_created", v, serializer),
            Self::SessionResumed(v) => serialize_event("session_resumed", v, serializer),
            Self::SessionClosed(v) => serialize_event("session_closed", v, serializer),
            Self::RunStarted(v) => serialize_event("run_started", v, serializer),
            Self::AgentStarted(v) => serialize_event("agent_started", v, serializer),
            Self::TurnStarted(v) => serialize_event("turn_started", v, serializer),
            Self::MessageStarted(v) => serialize_event("message_started", v, serializer),
            Self::MessageUpdated(v) => serialize_event("message_updated", v, serializer),
            Self::MessageCompleted(v) => serialize_event("message_completed", v, serializer),
            Self::ToolStarted(v) => serialize_event("tool_started", v, serializer),
            Self::ToolUpdated(v) => serialize_event("tool_updated", v, serializer),
            Self::ToolCompleted(v) => serialize_event("tool_completed", v, serializer),
            Self::Warning(v) => serialize_event("warning", v, serializer),
            Self::RouterDiagnostic(v) => serialize_event("router_diagnostic", v, serializer),
            Self::Retry(v) => serialize_event("retry", v, serializer),
            Self::ContextCompacted(v) => serialize_event("context_compacted", v, serializer),
            Self::ResourceLimit(v) => serialize_event("resource_limit", v, serializer),
            Self::SteeringInjected(v) => serialize_event("steering_injected", v, serializer),
            Self::RunCompleted(v) => serialize_event("run_completed", v, serializer),
            Self::CommandAck(v) => serialize_event("command_ack", v, serializer),
            Self::Error(v) => serialize_event("error", v, serializer),
            Self::SessionListed(v) => serialize_event("session_listed", v, serializer),
            Self::SessionAttached(v) => serialize_event("session_attached", v, serializer),
            Self::SessionDetached(v) => serialize_event("session_detached", v, serializer),
            Self::WorkListed(v) => serialize_event("work_listed", v, serializer),
            Self::WorkActioned(v) => serialize_event("work_actioned", v, serializer),
            Self::WorkChanged(v) => serialize_event("work_changed", v, serializer),
            Self::ApprovalRequested(v) => serialize_event("approval_requested", v, serializer),
            Self::QuestionAsked(v) => serialize_event("question_asked", v, serializer),
            Self::RunDrained(v) => serialize_event("run_drained", v, serializer),
            Self::SessionOwnershipReported(v) => {
                serialize_event("session_ownership_reported", v, serializer)
            }
            Self::Unknown { event_type, fields } => {
                let mut fields = fields.clone();
                fields.insert("type".to_owned(), Value::String(event_type.clone()));
                fields.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut fields = Map::<String, Value>::deserialize(deserializer)?;
        let event_type = fields
            .remove("type")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .ok_or_else(|| D::Error::custom("event.type must be a string"))?;
        let payload = Value::Object(fields.clone());

        macro_rules! decode {
            ($variant:ident, $ty:ty) => {
                serde_json::from_value::<$ty>(payload)
                    .map(Self::$variant)
                    .map_err(D::Error::custom)
            };
        }

        match event_type.as_str() {
            "runtime_ready" => decode!(RuntimeReady, RuntimeReady),
            "session_created" => decode!(SessionCreated, SessionEvent),
            "session_resumed" => decode!(SessionResumed, SessionEvent),
            "session_closed" => decode!(SessionClosed, SessionEvent),
            "run_started" => decode!(RunStarted, RunStarted),
            "agent_started" => decode!(AgentStarted, AgentStarted),
            "turn_started" => decode!(TurnStarted, TurnStarted),
            "message_started" => decode!(MessageStarted, MessageEvent),
            "message_updated" => decode!(MessageUpdated, MessageEvent),
            "message_completed" => decode!(MessageCompleted, MessageEvent),
            "tool_started" => decode!(ToolStarted, ToolEvent),
            "tool_updated" => decode!(ToolUpdated, ToolEvent),
            "tool_completed" => decode!(ToolCompleted, ToolEvent),
            "warning" => decode!(Warning, WarningEvent),
            "router_diagnostic" => decode!(RouterDiagnostic, RouterDiagnostic),
            "retry" => decode!(Retry, RetryEvent),
            "context_compacted" => decode!(ContextCompacted, ContextCompacted),
            "resource_limit" => decode!(ResourceLimit, ResourceLimit),
            "steering_injected" => decode!(SteeringInjected, SteeringInjected),
            "run_completed" => decode!(RunCompleted, RunCompleted),
            "command_ack" => decode!(CommandAck, CommandAck),
            "error" => decode!(Error, ErrorEvent),
            "session_listed" => decode!(SessionListed, SessionListed),
            "session_attached" => decode!(SessionAttached, SessionEvent),
            "session_detached" => decode!(SessionDetached, SessionEvent),
            "work_listed" => decode!(WorkListed, WorkListed),
            "work_actioned" => decode!(WorkActioned, WorkActioned),
            "work_changed" => decode!(WorkChanged, WorkChanged),
            "approval_requested" => decode!(ApprovalRequested, ApprovalRequested),
            "question_asked" => decode!(QuestionAsked, QuestionAsked),
            "run_drained" => decode!(RunDrained, RunDrained),
            "session_ownership_reported" => {
                decode!(SessionOwnershipReported, SessionOwnershipReported)
            }
            _ => Ok(Self::Unknown { event_type, fields }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_uses_v1_shape() {
        let request = RequestEnvelope::new(
            "req-1",
            Command::RunStart {
                session_id: "session-1".into(),
                prompt: "hello".into(),
                output_schema: None,
                max_turns: Some(3),
                token_budget: None,
                timeout_secs: None,
            },
        );
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["command"]["type"], "run_start");
        assert_eq!(value["command"]["prompt"], "hello");
        assert!(value["command"].get("output_schema").is_none());
    }

    #[test]
    fn event_round_trip_and_unknown_event_are_forward_compatible() {
        let known = Event::RunCompleted(RunCompleted {
            status: RunStatus::Completed,
            output: "done".into(),
            structured_output: Some(serde_json::json!({"answer": 42})),
            messages: vec![],
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
        });
        let value = serde_json::to_value(&known).unwrap();
        assert_eq!(value["type"], "run_completed");
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), known);

        // Additive fields on known v1 events are ignored for compatibility.
        let ready: Event = serde_json::from_value(serde_json::json!({
            "type": "runtime_ready",
            "runtime_version": "1.0.0",
            "supported_protocol_versions": [1],
            "capabilities": [],
            "future_additive_field": {"enabled": true}
        }))
        .unwrap();
        assert!(matches!(ready, Event::RuntimeReady(_)));

        let unknown: Event = serde_json::from_value(serde_json::json!({
            "type": "future_event",
            "new_field": true
        }))
        .unwrap();
        assert!(matches!(
            unknown,
            Event::Unknown { ref event_type, ref fields }
                if event_type == "future_event" && fields["new_field"] == true
        ));
    }

    /// Every daemon command (spec daemon-core R2.3) serializes under the v1
    /// tagged shape and round-trips through the shared enum.
    #[test]
    fn daemon_commands_round_trip() {
        let commands = vec![
            Command::SessionList,
            Command::SessionAttach {
                session_id: "s1".into(),
            },
            Command::SessionDetach {
                session_id: "s1".into(),
            },
            Command::WorkList,
            Command::WorkAction {
                work_id: "task:1".into(),
                action: WorkActionKind::Message {
                    text: "carry on".into(),
                },
            },
            Command::WorkAction {
                work_id: "agent-1".into(),
                action: WorkActionKind::Rename {
                    name: "reviewer".into(),
                },
            },
            Command::ApprovalRespond {
                request_id: "req-1".into(),
                decision: ApprovalDecision::Approve,
            },
            Command::QuestionAnswer {
                request_id: "req-2".into(),
                answer: "yes".into(),
            },
            Command::RunDrain {
                session_id: "s1".into(),
                now: true,
            },
            Command::SessionOwnership {
                session_id: "s1".into(),
                owner: OwnerWire::Platform,
                owner_generation: 3,
            },
        ];
        for command in commands {
            let request = RequestEnvelope::new("r", command.clone());
            let json = serde_json::to_value(&request).unwrap();
            assert!(json["command"]["type"].is_string(), "{json}");
            let back: RequestEnvelope = serde_json::from_value(json).unwrap();
            assert_eq!(back.command, command);
        }

        let stop = serde_json::to_value(Command::WorkAction {
            work_id: "w".into(),
            action: WorkActionKind::Stop,
        })
        .unwrap();
        assert_eq!(stop["type"], "work_action");
        assert_eq!(stop["action"]["kind"], "stop");
    }

    /// Every daemon event round-trips through the typed enum, and durations
    /// travel as elapsed milliseconds (R4.2), never as instants.
    #[test]
    fn daemon_events_round_trip() {
        let item = WorkItemWire {
            id: "task:7".into(),
            name: "cargo test".into(),
            kind: "bash_task".into(),
            status: "running".into(),
            owner: WorkOwnerWire::Session {
                session_id: "s1".into(),
            },
            parent: Some("main:s1".into()),
            elapsed_ms: 4_200,
            tokens_in: 12,
            tokens_out: 30,
            tool_calls: 3,
            current_action: Some("cargo test --workspace".into()),
            pending: Some(PendingWire {
                request_id: "req-9".into(),
                kind: "approval".into(),
                summary: Some("run cargo clippy".into()),
            }),
            detail: None,
            stalled_secs: None,
        };
        let events = vec![
            Event::SessionListed(SessionListed {
                sessions: vec![SessionSummary {
                    session_id: "s1".into(),
                    session_uuid: Some("8f0e7f5d-2a1a-4b3b-9d3a-0d2f6a1e2c33".into()),
                    cwd: "/tmp/p".into(),
                    model: Some("anthropic/claude".into()),
                    title: Some("fix login".into()),
                    created_at: "2026-09-09T00:00:00Z".into(),
                    last_activity_at: "2026-09-09T00:01:00Z".into(),
                    active_run: Some("run-1".into()),
                    attached_clients: 2,
                    owner: OwnerWire::Local,
                    owner_generation: 0,
                }],
            }),
            Event::SessionAttached(SessionEvent::default()),
            Event::SessionDetached(SessionEvent::default()),
            Event::WorkListed(WorkListed {
                revision: 41,
                counts: WorkCountsWire {
                    needs_input: 1,
                    active: 2,
                    finished: 0,
                },
                items: vec![item],
                truncated: false,
            }),
            Event::WorkActioned(WorkActioned {
                work_id: "task:7".into(),
            }),
            Event::WorkChanged(WorkChanged { revision: 42 }),
            Event::ApprovalRequested(ApprovalRequested {
                request_id: "req-9".into(),
                work_id: Some("agent-1".into()),
                tool: "bash".into(),
                summary: "run cargo clippy".into(),
                risk: Some("high".into()),
                always_allow_available: false,
            }),
            Event::QuestionAsked(QuestionAsked {
                request_id: "req-10".into(),
                work_id: None,
                question: "Which branch?".into(),
                options: vec!["main".into(), "dev".into()],
            }),
            Event::RunDrained(RunDrained {
                journal_sequence: 17,
                journal_hash: "abc".into(),
            }),
            Event::SessionOwnershipReported(SessionOwnershipReported {
                owner: OwnerWire::Platform,
                owner_generation: 3,
            }),
        ];
        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            let tag = json["type"].as_str().unwrap().to_owned();
            let back: Event = serde_json::from_value(json).unwrap();
            assert_eq!(back, event, "{tag}");
            assert!(
                !matches!(back, Event::Unknown { .. }),
                "{tag} must decode to a typed variant"
            );
        }
    }

    /// A peer built before these additions must see every new event as
    /// `Unknown` and keep going, and must be able to send the old commands
    /// unchanged (R2.3 additivity).
    #[test]
    fn v1_client_ignores_unknown_daemon_events() {
        for tag in [
            "session_listed",
            "session_attached",
            "session_detached",
            "work_listed",
            "work_actioned",
            "work_changed",
            "approval_requested",
            "question_asked",
            "run_drained",
            "session_ownership_reported",
        ] {
            // Emulate the pre-daemon decoder: only the original tags are known.
            let is_v1_tag = matches!(
                tag,
                "runtime_ready"
                    | "session_created"
                    | "session_resumed"
                    | "session_closed"
                    | "run_started"
                    | "turn_started"
                    | "message"
                    | "tool"
                    | "warning"
                    | "router_diagnostic"
                    | "retry"
                    | "context_compacted"
                    | "resource_limit"
                    | "steering_injected"
                    | "run_completed"
                    | "command_ack"
                    | "error"
            );
            assert!(!is_v1_tag, "{tag} must be a new tag");
        }

        let ready: RuntimeReady = serde_json::from_value(serde_json::json!({
            "runtime_version": "0.1.0",
            "supported_protocol_versions": [1],
            "capabilities": ["session_list", "work_registry"],
            "daemon": {"enabled": true}
        }))
        .unwrap();
        assert_eq!(ready.daemon, Some(DaemonInfo { enabled: true }));

        let legacy: RuntimeReady = serde_json::from_value(serde_json::json!({
            "runtime_version": "0.1.0",
            "supported_protocol_versions": [1]
        }))
        .unwrap();
        assert_eq!(legacy.daemon, None);
    }

    #[test]
    fn mcp_debug_redacts_secret_values() {
        let mcp = McpServerConfig::Sse {
            name: "server".into(),
            url: "https://token@example.invalid/?key=secret".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer secret".into())]),
        };
        let debug = format!("{mcp:?}");
        assert!(!debug.contains("Bearer secret"));
        assert!(!debug.contains("token@example"));
        assert!(debug.contains("Authorization"));

        let value = serde_json::to_value(mcp).unwrap();
        assert_eq!(value["transport"], "sse");
        assert_eq!(value["headers"]["Authorization"], "Bearer secret");
    }

    #[test]
    fn mcp_streamable_http_round_trips_and_redacts() {
        let mcp = McpServerConfig::StreamableHttp {
            name: "server".into(),
            url: "https://token@example.invalid/mcp".into(),
            headers: BTreeMap::from([("X-Static".into(), "static-secret".into())]),
            bearer_token_env_var: Some("MCP_TOKEN".into()),
            env_http_headers: BTreeMap::from([("X-Org".into(), "MCP_ORG".into())]),
        };

        // Debug never shows the URL or header values; env var *names* are
        // safe to print because they carry no secret material.
        let debug = format!("{mcp:?}");
        assert!(!debug.contains("static-secret"));
        assert!(!debug.contains("token@example"));
        assert!(debug.contains("MCP_TOKEN"));
        assert!(debug.contains("X-Static"));

        let value = serde_json::to_value(&mcp).unwrap();
        assert_eq!(value["transport"], "streamable_http");
        assert_eq!(value["bearer_token_env_var"], "MCP_TOKEN");
        assert_eq!(value["env_http_headers"]["X-Org"], "MCP_ORG");
        let back: McpServerConfig = serde_json::from_value(value).unwrap();
        assert_eq!(back, mcp);

        // Optional auth fields are omitted on the wire when unset, so older
        // consumers see the same shape as before.
        let bare = serde_json::to_value(McpServerConfig::StreamableHttp {
            name: "bare".into(),
            url: "https://example.invalid/mcp".into(),
            headers: BTreeMap::new(),
            bearer_token_env_var: None,
            env_http_headers: BTreeMap::new(),
        })
        .unwrap();
        assert!(bare.get("bearer_token_env_var").is_none());
        assert!(bare.get("env_http_headers").is_none());
    }

    #[test]
    fn policy_serializes_exact_field_names() {
        let policy = ExecutionPolicy {
            allowed_tools: vec!["read".into()],
            denied_tools: vec!["shell".into()],
            sandbox_mode: SandboxMode::WorkspaceWrite,
            sandbox_config: Some(serde_json::json!({"writable_roots": ["/tmp"]})),
            allowed_domains: vec!["example.com".into()],
            interactive_approvals: false,
        };
        let value = serde_json::to_value(policy).unwrap();
        assert_eq!(value["allowed_tools"][0], "read");
        assert_eq!(value["denied_tools"][0], "shell");
        assert_eq!(value["sandbox_mode"], "workspace_write");
        assert_eq!(value["allowed_domains"][0], "example.com");
    }
}
