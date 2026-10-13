//! Async control-plane SDK for a standalone `o4-agent-runtime`.
//!
//! This crate launches or connects to a runtime, sends protocol-v1 JSONL
//! commands, and routes typed events. It does not implement an agent loop.
//! [`Transport`] is byte-stream agnostic and has no PTY or sshable dependency.

mod client;
mod error;
mod transport;

pub use client::{
    ApprovalOutcome, ApprovalRequest, ApprovalStream, AttachedSession, Client, ClientOptions,
    ResumeOptions, RunHandle, RunOptions, RunResult, RuntimeInfo, Session, SessionOptions,
    WorkWatcher,
};
pub use error::{Error, Result, RuntimeDiagnostics};
pub use o4_agent_protocol as protocol;
pub use o4_agent_protocol::{
    ApprovalDecision, ApprovalRequested, Context, Event, EventEnvelope, ExecutionPolicy,
    McpServerConfig, Reasoning, RunStatus, SandboxMode, SessionSummary, Usage, WorkActionKind,
    WorkChanged, WorkCountsWire, WorkItemWire, WorkListed, PROTOCOL_VERSION, SDK_CAPABILITIES,
    SUPPORTED_PROTOCOL_VERSIONS,
};
pub use transport::{LocalRuntime, Transport, TransportError, TransportSource};

/// Runtime versions compatible with this SDK release by default.
///
/// Every 0.x runtime: o4's minor number moves when its patch number passes
/// 99, not on a protocol change, so a per-minor ceiling only cuts users off
/// at each rollover. Protocol version and capabilities are the real checks.
///
/// Keep this in step with `runtime_version_requirement` in
/// `compatibility.toml`; `compatibility_pin_matches_compiled_default` fails if
/// the two drift apart.
pub const DEFAULT_RUNTIME_VERSION_REQUIREMENT: &str = ">=0.0.57, <1.0.0";
