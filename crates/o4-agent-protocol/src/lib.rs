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
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
            sandbox_mode: SandboxMode::ReadOnly,
            sandbox_config: None,
            allowed_domains: Vec::new(),
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeReady {
    pub runtime_version: String,
    pub supported_protocol_versions: Vec<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
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
    fn policy_serializes_exact_field_names() {
        let policy = ExecutionPolicy {
            allowed_tools: vec!["read".into()],
            denied_tools: vec!["shell".into()],
            sandbox_mode: SandboxMode::WorkspaceWrite,
            sandbox_config: Some(serde_json::json!({"writable_roots": ["/tmp"]})),
            allowed_domains: vec!["example.com".into()],
        };
        let value = serde_json::to_value(policy).unwrap();
        assert_eq!(value["allowed_tools"][0], "read");
        assert_eq!(value["denied_tools"][0], "shell");
        assert_eq!(value["sandbox_mode"], "workspace_write");
        assert_eq!(value["allowed_domains"][0], "example.com");
    }
}
