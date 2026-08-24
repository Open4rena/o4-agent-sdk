use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::error::{Error, Result, RuntimeDiagnostics};

const DEFAULT_STDERR_LIMIT: usize = 64 * 1024;
const PROCESS_EXIT_GRACE: Duration = Duration::from_secs(5);

/// A transport-level failure. Implementations should avoid placing secrets in
/// the message because it can be displayed through the public SDK error.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct TransportError {
    message: String,
}

impl TransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Bidirectional, line-oriented transport for the runtime wire protocol.
///
/// The client has exactly one receive task, may have concurrent senders, and
/// passes JSON without a trailing newline. Implementations must frame each
/// send as one complete line and return one complete line per receive. The
/// interface deliberately contains no terminal or PTY operations, allowing a
/// future remote/sshable adapter to bridge ordinary byte streams.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn send_line(&self, line: String) -> std::result::Result<(), TransportError>;

    /// Returns `None` at permanent EOF.
    async fn receive_line(&self) -> std::result::Result<Option<String>, TransportError>;

    /// Gracefully close transport resources after the runtime acknowledged
    /// `shutdown`.
    async fn close(&self) -> std::result::Result<(), TransportError> {
        Ok(())
    }

    /// Synchronous best-effort fallback used when a client is dropped without
    /// an explicit shutdown.
    fn abort(&self) {}

    /// Return bounded process diagnostics. Values may be sensitive and are
    /// redacted by their `Debug` implementation.
    fn diagnostics(&self) -> RuntimeDiagnostics {
        RuntimeDiagnostics::default()
    }
}

/// A value that can provide a connected [`Transport`].
#[async_trait]
pub trait TransportSource: Send + 'static {
    async fn open(self) -> Result<Arc<dyn Transport>>;
}

#[async_trait]
impl<T> TransportSource for T
where
    T: Transport,
{
    async fn open(self) -> Result<Arc<dyn Transport>> {
        Ok(Arc::new(self))
    }
}

#[async_trait]
impl TransportSource for Arc<dyn Transport> {
    async fn open(self) -> Result<Arc<dyn Transport>> {
        Ok(self)
    }
}

/// Configuration that launches a local `o4-agent-runtime` with piped standard
/// input, standard output, and standard error.
///
/// The runtime receives no SDK-controlled shell and no PTY.
#[derive(Clone)]
pub struct LocalRuntime {
    path: PathBuf,
    stderr_limit: usize,
}

impl LocalRuntime {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            stderr_limit: DEFAULT_STDERR_LIMIT,
        }
    }

    pub fn stderr_limit(mut self, bytes: usize) -> Self {
        self.stderr_limit = bytes;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Debug for LocalRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalRuntime")
            .field("path", &self.path)
            .field("stderr_limit", &self.stderr_limit)
            .finish()
    }
}

#[async_trait]
impl TransportSource for LocalRuntime {
    async fn open(self) -> Result<Arc<dyn Transport>> {
        let runtime_path = validate_runtime_path(&self.path)?;

        let mut command = Command::new(runtime_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|error| Error::Transport {
            message: format!("could not launch runtime: {error}"),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| Error::Transport {
            message: "runtime stdin was not piped".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| Error::Transport {
            message: "runtime stdout was not piped".into(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| Error::Transport {
            message: "runtime stderr was not piped".into(),
        })?;

        let diagnostics = Arc::new(Mutex::new(BoundedBytes::new(self.stderr_limit)));
        let stderr_diagnostics = Arc::clone(&diagnostics);
        let stderr_task = tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr);
            let mut chunk = [0_u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => lock(&stderr_diagnostics).push(&chunk[..read]),
                }
            }
        });

        Ok(Arc::new(LocalTransport {
            writer: AsyncMutex::new(Some(BufWriter::new(stdin))),
            reader: AsyncMutex::new(Some(BufReader::new(stdout).lines())),
            child: AsyncMutex::new(Some(child)),
            diagnostics,
            stderr_task: Mutex::new(Some(stderr_task)),
        }))
    }
}

fn validate_runtime_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(Error::InvalidRuntimePath {
            path: path.to_owned(),
            reason: "path is empty".into(),
        });
    }
    // Resolve first so the launched file is the one that was validated. This
    // also rejects dangling symlinks and avoids PATH-based executable lookup.
    let runtime_path = std::fs::canonicalize(path).map_err(|error| Error::InvalidRuntimePath {
        path: path.to_owned(),
        reason: error.to_string(),
    })?;
    let metadata = std::fs::metadata(&runtime_path).map_err(|error| Error::InvalidRuntimePath {
        path: path.to_owned(),
        reason: error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(Error::InvalidRuntimePath {
            path: path.to_owned(),
            reason: "path is not a file".into(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::InvalidRuntimePath {
                path: path.to_owned(),
                reason: "file is not executable".into(),
            });
        }
    }
    Ok(runtime_path)
}

struct LocalTransport {
    writer: AsyncMutex<Option<BufWriter<ChildStdin>>>,
    reader: AsyncMutex<Option<Lines<BufReader<ChildStdout>>>>,
    child: AsyncMutex<Option<Child>>,
    diagnostics: Arc<Mutex<BoundedBytes>>,
    stderr_task: Mutex<Option<JoinHandle<()>>>,
}

#[async_trait]
impl Transport for LocalTransport {
    async fn send_line(&self, line: String) -> std::result::Result<(), TransportError> {
        if line.contains('\n') || line.contains('\r') {
            return Err(TransportError::new(
                "serialized request contained an unescaped line break",
            ));
        }
        let mut writer = self.writer.lock().await;
        let writer = writer
            .as_mut()
            .ok_or_else(|| TransportError::new("runtime input is closed"))?;
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|error| TransportError::new(format!("could not write request: {error}")))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|error| TransportError::new(format!("could not frame request: {error}")))?;
        writer
            .flush()
            .await
            .map_err(|error| TransportError::new(format!("could not flush request: {error}")))
    }

    async fn receive_line(&self) -> std::result::Result<Option<String>, TransportError> {
        let mut reader = self.reader.lock().await;
        let Some(reader) = reader.as_mut() else {
            return Ok(None);
        };
        reader
            .next_line()
            .await
            .map_err(|error| TransportError::new(format!("could not read runtime output: {error}")))
    }

    async fn close(&self) -> std::result::Result<(), TransportError> {
        self.writer.lock().await.take();

        let mut child = self.child.lock().await.take();
        if let Some(mut child) = child.take() {
            match tokio::time::timeout(PROCESS_EXIT_GRACE, child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    return Err(TransportError::new(format!(
                        "could not wait for runtime exit: {error}"
                    )))
                }
                Err(_) => {
                    child.start_kill().map_err(|error| {
                        TransportError::new(format!("could not stop runtime: {error}"))
                    })?;
                    let _ = child.wait().await;
                }
            }
        }

        let stderr_task = lock(&self.stderr_task).take();
        if let Some(task) = stderr_task {
            let _ = task.await;
        }
        Ok(())
    }

    fn abort(&self) {
        if let Ok(mut child) = self.child.try_lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
        }
        if let Some(task) = lock(&self.stderr_task).take() {
            task.abort();
        }
    }

    fn diagnostics(&self) -> RuntimeDiagnostics {
        RuntimeDiagnostics::new(lock(&self.diagnostics).to_string_lossy())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct BoundedBytes {
    bytes: VecDeque<u8>,
    limit: usize,
}

impl BoundedBytes {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit.min(4096)),
            limit,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.limit == 0 {
            return;
        }
        if bytes.len() >= self.limit {
            self.bytes.clear();
            self.bytes.extend(
                bytes[bytes.len().saturating_sub(self.limit)..]
                    .iter()
                    .copied(),
            );
            return;
        }
        let excess = self
            .bytes
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(self.limit);
        self.bytes.drain(..excess);
        self.bytes.extend(bytes.iter().copied());
    }

    fn to_string_lossy(&self) -> String {
        let bytes: Vec<_> = self.bytes.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_bytes_keep_only_the_tail() {
        let mut bytes = BoundedBytes::new(5);
        bytes.push(b"abc");
        bytes.push(b"defg");
        assert_eq!(bytes.to_string_lossy(), "cdefg");
        bytes.push(b"123456");
        assert_eq!(bytes.to_string_lossy(), "23456");
    }
}
