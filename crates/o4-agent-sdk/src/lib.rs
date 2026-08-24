//! Async control-plane SDK for a standalone `o4-agent-runtime`.
//!
//! This crate launches or connects to a runtime, sends protocol-v1 JSONL
//! commands, and routes typed events. It does not implement an agent loop.
//! [`Transport`] is byte-stream agnostic and has no PTY or sshable dependency.

mod client;
mod error;
mod transport;

pub use client::{
    Client, ClientOptions, ResumeOptions, RunHandle, RunOptions, RunResult, RuntimeInfo, Session,
    SessionOptions,
};
pub use error::{Error, Result, RuntimeDiagnostics};
pub use o4_agent_protocol as protocol;
pub use o4_agent_protocol::{
    Context, Event, EventEnvelope, ExecutionPolicy, McpServerConfig, Reasoning, RunStatus,
    SandboxMode, Usage, DEFAULT_RUNTIME_VERSION_REQUIREMENT, PROTOCOL_VERSION, SDK_CAPABILITIES,
    SUPPORTED_PROTOCOL_VERSIONS,
};
pub use transport::{LocalRuntime, Transport, TransportError, TransportSource};
