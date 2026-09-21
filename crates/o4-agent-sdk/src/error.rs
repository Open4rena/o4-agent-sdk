use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// Bounded stderr captured from a local runtime.
///
/// Its contents are deliberately omitted from `Debug` and `Display`, so
/// routine error logging cannot accidentally copy credentials printed by a
/// child process. Call [`RuntimeDiagnostics::as_str`] only in an explicitly
/// secured diagnostic path.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RuntimeDiagnostics(String);

impl RuntimeDiagnostics {
    /// Construct diagnostics captured by a custom transport.
    ///
    /// The value is retained verbatim but redacted from `Debug` and `Display`.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns captured stderr. Treat this value as sensitive.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RuntimeDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RuntimeDiagnostics(<{} bytes redacted>)", self.0.len())
    }
}

/// Errors returned by the SDK.
#[derive(Clone, Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum Error {
    #[error("runtime executable path is invalid: {path} ({reason})")]
    InvalidRuntimePath { path: PathBuf, reason: String },

    #[error("working directory is invalid: {path} ({reason})")]
    InvalidWorkingDirectory { path: PathBuf, reason: String },

    #[error("transport error: {message}")]
    Transport { message: String },

    #[error("failed to encode a protocol request")]
    Serialization,

    #[error("runtime emitted malformed protocol output: {reason}")]
    MalformedProtocol { reason: String },

    #[error("runtime violated the wire protocol: {reason}")]
    ProtocolViolation { reason: String },

    #[error(
        "no compatible protocol version (SDK supports {sdk_supported:?}, runtime supports {runtime_supported:?})"
    )]
    ProtocolVersionMismatch {
        sdk_supported: Vec<u32>,
        runtime_supported: Vec<u32>,
    },

    #[error("runtime version `{version}` is not valid semantic versioning")]
    InvalidRuntimeVersion { version: String },

    #[error("runtime version {version} does not satisfy {requirement}")]
    RuntimeVersionIncompatible {
        version: String,
        requirement: String,
    },

    #[error("runtime is missing required capabilities: {missing:?}")]
    MissingCapabilities { missing: Vec<String> },

    /// A daemon-surface call was made against a runtime that does not advertise
    /// the capability backing it.
    ///
    /// Raised before anything is written to the transport, so it can never be
    /// confused with a runtime rejection or a connection failure. One variant
    /// carrying the capability name rather than one variant per capability:
    /// the runtime gains capabilities over time, and a new enum variant each
    /// time is a breaking change for anyone matching exhaustively.
    #[error("runtime does not advertise the `{capability}` capability")]
    MissingCapability { capability: String },

    #[error("runtime handshake timed out")]
    HandshakeTimeout,

    #[error("runtime exited unexpectedly (captured stderr is available explicitly)")]
    RuntimeExited { diagnostics: RuntimeDiagnostics },

    #[error("runtime rejected the command ({code}): {message}")]
    Runtime {
        code: String,
        message: String,
        retryable: bool,
    },

    #[error("client has been shut down")]
    ClientShutdown,

    #[error("session `{session_id}` is closed")]
    SessionClosed { session_id: String },

    #[error("run `{run_id}` is no longer active")]
    RunFinished { run_id: String },

    #[error("event consumer fell behind the runtime")]
    EventBufferOverflow,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
