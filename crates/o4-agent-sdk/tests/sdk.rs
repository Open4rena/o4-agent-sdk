use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use o4_agent_sdk::protocol::{
    AgentStarted, Command, CommandAck, Context, Event, EventEnvelope, ExecutionPolicy,
    McpServerConfig, MessageEvent, RequestEnvelope, RunCompleted, RunStarted, RunStatus,
    RuntimeReady, SandboxMode, SessionEvent, SteeringInjected, Usage, WarningEvent,
};
use o4_agent_sdk::{
    Client, ClientOptions, Error, LocalRuntime, Reasoning, ResumeOptions, RuntimeDiagnostics,
    SessionOptions, Transport, TransportError, DEFAULT_RUNTIME_VERSION_REQUIREMENT,
    PROTOCOL_VERSION,
};
use semver::VersionReq;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

#[derive(Default)]
struct MemoryState {
    aborted: AtomicBool,
    closed: AtomicBool,
    sent_lines: Mutex<Vec<String>>,
}

struct MemoryTransport {
    requests: mpsc::UnboundedSender<String>,
    events: AsyncMutex<mpsc::UnboundedReceiver<Result<String, TransportError>>>,
    state: Arc<MemoryState>,
    diagnostics: RuntimeDiagnostics,
}

#[async_trait]
impl Transport for MemoryTransport {
    async fn send_line(&self, line: String) -> Result<(), TransportError> {
        self.state.sent_lines.lock().unwrap().push(line.clone());
        self.requests
            .send(line)
            .map_err(|_| TransportError::new("fake runtime stopped receiving"))
    }

    async fn receive_line(&self) -> Result<Option<String>, TransportError> {
        self.events.lock().await.recv().await.transpose()
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.state.closed.store(true, Ordering::Release);
        Ok(())
    }

    fn abort(&self) {
        self.state.aborted.store(true, Ordering::Release);
    }

    fn diagnostics(&self) -> RuntimeDiagnostics {
        self.diagnostics.clone()
    }
}

struct RuntimePeer {
    requests: mpsc::UnboundedReceiver<String>,
    events: mpsc::UnboundedSender<Result<String, TransportError>>,
    sequence: u64,
}

impl RuntimePeer {
    async fn request(&mut self) -> RequestEnvelope {
        let line = self
            .requests
            .recv()
            .await
            .expect("SDK unexpectedly closed its request stream");
        assert!(
            !line.contains('\n'),
            "Transport receives unframed JSON lines"
        );
        serde_json::from_str(&line).expect("SDK request must be valid protocol JSON")
    }

    fn send(
        &mut self,
        request_id: &str,
        session_id: Option<&str>,
        run_id: Option<&str>,
        event: Event,
    ) {
        self.sequence += 1;
        self.send_at(self.sequence, request_id, session_id, run_id, event);
    }

    fn send_at(
        &self,
        sequence: u64,
        request_id: &str,
        session_id: Option<&str>,
        run_id: Option<&str>,
        event: Event,
    ) {
        let envelope = EventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            sequence,
            timestamp: "2025-01-02T03:04:05Z".into(),
            runtime_id: "fake-runtime".into(),
            session_id: session_id.map(ToOwned::to_owned),
            run_id: run_id.map(ToOwned::to_owned),
            context: None,
            event,
        };
        self.events
            .send(Ok(serde_json::to_string(&envelope).unwrap()))
            .expect("SDK reader unexpectedly stopped");
    }

    fn send_raw(&self, line: impl Into<String>) {
        let _ = self.events.send(Ok(line.into()));
    }
}

fn memory_pair(diagnostics: impl Into<String>) -> (MemoryTransport, RuntimePeer, Arc<MemoryState>) {
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let state = Arc::new(MemoryState::default());
    (
        MemoryTransport {
            requests: request_tx,
            events: AsyncMutex::new(event_rx),
            state: Arc::clone(&state),
            diagnostics: RuntimeDiagnostics::new(diagnostics),
        },
        RuntimePeer {
            requests: request_rx,
            events: event_tx,
            sequence: 0,
        },
        state,
    )
}

async fn handshake_with_version(peer: &mut RuntimePeer, runtime_version: &str) {
    let request = peer.request().await;
    assert_eq!(request.protocol_version, PROTOCOL_VERSION);
    match request.command {
        Command::Initialize {
            client_name,
            client_version,
            supported_protocol_versions,
            capabilities,
        } => {
            assert!(!client_name.is_empty());
            assert!(!client_version.is_empty());
            assert_eq!(supported_protocol_versions, vec![PROTOCOL_VERSION]);
            assert!(capabilities.iter().any(|value| value == "event.unknown"));
        }
        command => panic!("expected initialize, received {command:?}"),
    }
    peer.send(
        &request.request_id,
        None,
        None,
        Event::RuntimeReady(RuntimeReady {
            runtime_version: runtime_version.into(),
            supported_protocol_versions: vec![PROTOCOL_VERSION],
            capabilities: vec![
                "event.unknown".into(),
                "run.cancel".into(),
                "run.steer".into(),
                "session.resume".into(),
            ],
        }),
    );
}

async fn handshake(peer: &mut RuntimePeer) {
    handshake_with_version(peer, "0.0.57").await;
}

async fn auto_runtime(mut peer: RuntimePeer, commands: Arc<Mutex<Vec<Command>>>) {
    handshake(&mut peer).await;
    let mut next_session = 1_u32;
    let mut next_run = 1_u32;
    while let Some(line) = peer.requests.recv().await {
        assert!(!line.contains('\n'));
        let request: RequestEnvelope = serde_json::from_str(&line).unwrap();
        commands.lock().unwrap().push(request.command.clone());
        match request.command {
            Command::SessionCreate { session_id, .. } => {
                let session_id = session_id.unwrap_or_else(|| {
                    let value = format!("session-{next_session}");
                    next_session += 1;
                    value
                });
                peer.send(
                    &request.request_id,
                    Some(&session_id),
                    None,
                    Event::SessionCreated(SessionEvent::default()),
                );
            }
            Command::SessionResume { session_id, .. } => peer.send(
                &request.request_id,
                Some(&session_id),
                None,
                Event::SessionResumed(SessionEvent::default()),
            ),
            Command::RunStart {
                session_id, prompt, ..
            } => {
                let run_id = format!("run-{next_run}");
                next_run += 1;
                peer.send(
                    &request.request_id,
                    Some(&session_id),
                    Some(&run_id),
                    Event::RunStarted(RunStarted::default()),
                );
                peer.send(
                    &request.request_id,
                    Some(&session_id),
                    Some(&run_id),
                    Event::AgentStarted(AgentStarted {
                        agent_id: Some("agent-1".into()),
                    }),
                );
                peer.send(
                    &request.request_id,
                    Some(&session_id),
                    Some(&run_id),
                    Event::MessageCompleted(MessageEvent {
                        message: json!({"role": "assistant", "content": format!("answer: {prompt}")}),
                    }),
                );
                peer.send(
                    &request.request_id,
                    Some(&session_id),
                    Some(&run_id),
                    Event::RunCompleted(completed(format!("answer: {prompt}"))),
                );
            }
            Command::RunCancel { .. } | Command::RunSteer { .. } => peer.send(
                &request.request_id,
                None,
                None,
                Event::CommandAck(CommandAck::default()),
            ),
            Command::SessionClose { session_id } => peer.send(
                &request.request_id,
                Some(&session_id),
                None,
                Event::SessionClosed(SessionEvent::default()),
            ),
            Command::Shutdown => {
                peer.send(
                    &request.request_id,
                    None,
                    None,
                    Event::CommandAck(CommandAck {
                        command: Some("shutdown".into()),
                    }),
                );
                break;
            }
            Command::Initialize { .. } => panic!("initialize was sent more than once"),
        }
    }
}

fn completed(output: String) -> RunCompleted {
    RunCompleted {
        status: RunStatus::Completed,
        output,
        structured_output: Some(json!({"ok": true})),
        messages: vec![json!({"role": "assistant", "content": "done"})],
        usage: Usage {
            input_tokens: 4,
            output_tokens: 5,
            total_tokens: 9,
            cached_input_tokens: 1,
            cost_usd: Some(0.001),
        },
        turns: 1,
        tool_calls: 0,
    }
}

fn cwd() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
async fn handshake_rejects_protocol_version_mismatch() {
    let (transport, mut peer, state) = memory_pair("");
    let runtime = tokio::spawn(async move {
        let request = peer.request().await;
        assert!(matches!(request.command, Command::Initialize { .. }));
        peer.send(
            &request.request_id,
            None,
            None,
            Event::RuntimeReady(RuntimeReady {
                runtime_version: "1.0.0".into(),
                supported_protocol_versions: vec![2],
                capabilities: vec![],
            }),
        );
    });

    let error = Client::connect(transport).await.unwrap_err();
    assert!(matches!(
        error,
        Error::ProtocolVersionMismatch {
            sdk_supported,
            runtime_supported
        } if sdk_supported == vec![1] && runtime_supported == vec![2]
    ));
    runtime.await.unwrap();
    assert!(state.aborted.load(Ordering::Acquire));
}

#[tokio::test]
async fn default_runtime_requirement_accepts_pinned_runtime() {
    let (transport, peer, _) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, Arc::clone(&commands)));

    let client = Client::connect(transport).await.unwrap();
    assert_eq!(client.runtime_info().runtime_version.to_string(), "0.0.57");
    client.shutdown().await.unwrap();
    runtime.await.unwrap();
}

#[tokio::test]
async fn default_runtime_requirement_rejects_older_and_next_major_runtimes() {
    for version in ["0.0.56", "0.3.0"] {
        let (transport, mut peer, _) = memory_pair("");
        let runtime_version = version.to_owned();
        let runtime =
            tokio::spawn(async move { handshake_with_version(&mut peer, &runtime_version).await });

        let error = Client::connect(transport).await.unwrap_err();
        assert!(matches!(
            error,
            Error::RuntimeVersionIncompatible {
                version: rejected,
                requirement
            } if rejected == version
                && requirement.contains(">=0.0.57")
                && requirement.contains("<0.3.0")
        ));
        runtime.await.unwrap();
    }
}

#[tokio::test]
async fn runtime_semver_range_and_capabilities_are_separate_checks() {
    let (transport, mut peer, _) = memory_pair("");
    let runtime = tokio::spawn(async move { handshake(&mut peer).await });
    let options = ClientOptions::default()
        .runtime_version_requirement(VersionReq::parse(">=2.0.0, <3.0.0").unwrap());
    let error = Client::connect_with_options(transport, options)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::RuntimeVersionIncompatible { version, requirement }
            if version == "0.0.57" && requirement.contains(">=2.0.0")
    ));
    runtime.await.unwrap();

    let (transport, mut peer, _) = memory_pair("");
    let runtime = tokio::spawn(async move { handshake(&mut peer).await });
    let error = Client::connect_with_options(
        transport,
        ClientOptions::default().require_capability("future.required"),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        Error::MissingCapabilities { missing } if missing == vec!["future.required"]
    ));
    runtime.await.unwrap();
}

#[tokio::test]
async fn one_shot_buffers_result_and_unloads_session() {
    let (transport, peer, state) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, Arc::clone(&commands)));
    let client = Client::connect(transport).await.unwrap();

    assert_eq!(client.runtime_info().protocol_version, PROTOCOL_VERSION);
    let result = client
        .run("hello", SessionOptions::new("o4-test", cwd()))
        .await
        .unwrap();
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.output, "answer: hello");
    assert_eq!(result.usage.total_tokens, 9);

    client.shutdown().await.unwrap();
    runtime.await.unwrap();
    let commands = commands.lock().unwrap();
    assert!(matches!(commands[0], Command::SessionCreate { .. }));
    assert!(matches!(commands[1], Command::RunStart { .. }));
    assert!(matches!(commands[2], Command::SessionClose { .. }));
    assert!(matches!(commands[3], Command::Shutdown));
    assert!(state.closed.load(Ordering::Acquire));
    assert!(!state.aborted.load(Ordering::Acquire));
}

#[tokio::test]
async fn dropping_client_without_shutdown_uses_abort_fallback() {
    let (transport, mut peer, state) = memory_pair("");
    let runtime = tokio::spawn(async move {
        handshake(&mut peer).await;
        // Keep the runtime side alive until the test task is aborted.
        std::future::pending::<()>().await;
    });
    let client = Client::connect(transport).await.unwrap();
    drop(client);
    tokio::task::yield_now().await;
    assert!(state.aborted.load(Ordering::Acquire));
    assert!(!state.closed.load(Ordering::Acquire));
    runtime.abort();
    let _ = runtime.await;
}

#[tokio::test]
async fn persistent_multi_turn_and_resume_construct_expected_commands() {
    let (transport, peer, _) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, Arc::clone(&commands)));
    let client = Client::connect(transport).await.unwrap();
    let context = Context {
        workspace_id: Some("workspace-a".into()),
        user_id: Some("user-a".into()),
        correlation_id: Some("correlation-a".into()),
    };
    let session = client
        .create_session(
            SessionOptions::new("o4-test", cwd())
                .session_id("durable-1")
                .system_prompt("be concise")
                .reasoning(Reasoning::High)
                .context(context.clone()),
        )
        .await
        .unwrap();
    assert_eq!(session.id(), "durable-1");
    assert_eq!(
        session
            .run("turn one")
            .await
            .unwrap()
            .final_result()
            .await
            .unwrap()
            .output,
        "answer: turn one"
    );
    assert_eq!(
        session
            .run("turn two")
            .await
            .unwrap()
            .final_result()
            .await
            .unwrap()
            .output,
        "answer: turn two"
    );
    session.close().await.unwrap();

    let resumed = client
        .resume_session(
            "durable-1",
            ResumeOptions::default().cwd(cwd()).context(context.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        resumed
            .run("turn three")
            .await
            .unwrap()
            .final_result()
            .await
            .unwrap()
            .output,
        "answer: turn three"
    );
    resumed.close().await.unwrap();
    client.shutdown().await.unwrap();
    runtime.await.unwrap();

    let commands = commands.lock().unwrap();
    assert!(matches!(
        &commands[0],
        Command::SessionCreate {
            session_id: Some(id),
            model,
            system_prompt: Some(system_prompt),
            reasoning: Some(Reasoning::High),
            context: Some(received_context),
            ..
        } if id == "durable-1"
            && model == "o4-test"
            && system_prompt == "be concise"
            && received_context == &context
    ));
    let prompts: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            Command::RunStart { prompt, .. } => Some(prompt.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prompts, vec!["turn one", "turn two", "turn three"]);
    assert!(matches!(
        &commands[4],
        Command::SessionResume {
            session_id,
            cwd: Some(_),
            context: Some(received_context),
        } if session_id == "durable-1" && received_context == &context
    ));
}

#[tokio::test]
async fn one_reader_routes_interleaved_concurrent_runs() {
    let (transport, mut peer, _) = memory_pair("");
    let runtime = tokio::spawn(async move {
        handshake(&mut peer).await;
        let create = peer.request().await;
        peer.send(
            &create.request_id,
            Some("session-routes"),
            None,
            Event::SessionCreated(SessionEvent::default()),
        );

        let first = peer.request().await;
        let second = peer.request().await;
        let (first_prompt, second_prompt) = match (&first.command, &second.command) {
            (Command::RunStart { prompt: first, .. }, Command::RunStart { prompt: second, .. }) => {
                (first.clone(), second.clone())
            }
            _ => panic!("expected two run_start commands"),
        };
        peer.send(
            &first.request_id,
            Some("session-routes"),
            Some("run-a"),
            Event::RunStarted(RunStarted::default()),
        );
        peer.send(
            &second.request_id,
            Some("session-routes"),
            Some("run-b"),
            Event::RunStarted(RunStarted::default()),
        );
        peer.send(
            &second.request_id,
            Some("session-routes"),
            Some("run-b"),
            Event::MessageCompleted(MessageEvent {
                message: json!({"content": second_prompt}),
            }),
        );
        peer.send(
            &first.request_id,
            Some("session-routes"),
            Some("run-a"),
            Event::MessageCompleted(MessageEvent {
                message: json!({"content": first_prompt}),
            }),
        );
        peer.send(
            &second.request_id,
            Some("session-routes"),
            Some("run-b"),
            Event::RunCompleted(completed("second result".into())),
        );
        peer.send(
            &first.request_id,
            Some("session-routes"),
            Some("run-a"),
            Event::RunCompleted(completed("first result".into())),
        );

        let close = peer.request().await;
        peer.send(
            &close.request_id,
            Some("session-routes"),
            None,
            Event::SessionClosed(SessionEvent::default()),
        );
        let shutdown = peer.request().await;
        peer.send(
            &shutdown.request_id,
            None,
            None,
            Event::CommandAck(CommandAck::default()),
        );
    });

    let client = Client::connect(transport).await.unwrap();
    let session = client
        .create_session(SessionOptions::new("o4-test", cwd()))
        .await
        .unwrap();
    let first = session.run("first").await.unwrap();
    let second = session.run("second").await.unwrap();
    let (first, second) = tokio::join!(first.final_result(), second.final_result());
    assert_eq!(first.unwrap().output, "first result");
    assert_eq!(second.unwrap().output, "second result");
    session.close().await.unwrap();
    client.shutdown().await.unwrap();
    runtime.await.unwrap();
}

#[tokio::test]
async fn active_run_supports_steer_and_cancel() {
    let (transport, mut peer, _) = memory_pair("");
    let runtime = tokio::spawn(async move {
        handshake(&mut peer).await;
        let create = peer.request().await;
        peer.send(
            &create.request_id,
            Some("session-control"),
            None,
            Event::SessionCreated(SessionEvent::default()),
        );
        let run = peer.request().await;
        peer.send(
            &run.request_id,
            Some("session-control"),
            Some("run-control"),
            Event::RunStarted(RunStarted::default()),
        );

        let steer = peer.request().await;
        assert!(matches!(
            &steer.command,
            Command::RunSteer { run_id, instruction }
                if run_id == "run-control" && instruction == "focus on tests"
        ));
        peer.send(
            &steer.request_id,
            Some("session-control"),
            Some("run-control"),
            Event::CommandAck(CommandAck {
                command: Some("run_steer".into()),
            }),
        );

        let cancel = peer.request().await;
        assert!(matches!(
            &cancel.command,
            Command::RunCancel { run_id } if run_id == "run-control"
        ));
        peer.send(
            &cancel.request_id,
            Some("session-control"),
            Some("run-control"),
            Event::CommandAck(CommandAck {
                command: Some("run_cancel".into()),
            }),
        );
        peer.send(
            &run.request_id,
            Some("session-control"),
            Some("run-control"),
            Event::SteeringInjected(SteeringInjected {
                instruction: "focus on tests".into(),
            }),
        );
        let mut cancelled = completed(String::new());
        cancelled.status = RunStatus::Cancelled;
        peer.send(
            &run.request_id,
            Some("session-control"),
            Some("run-control"),
            Event::RunCompleted(cancelled),
        );

        let close = peer.request().await;
        peer.send(
            &close.request_id,
            Some("session-control"),
            None,
            Event::SessionClosed(SessionEvent::default()),
        );
        let shutdown = peer.request().await;
        peer.send(
            &shutdown.request_id,
            None,
            None,
            Event::CommandAck(CommandAck::default()),
        );
    });

    let client = Client::connect(transport).await.unwrap();
    let session = client
        .create_session(SessionOptions::new("o4-test", cwd()))
        .await
        .unwrap();
    let run = session.run("long task").await.unwrap();
    run.steer("focus on tests").await.unwrap();
    run.cancel().await.unwrap();
    let result = run.final_result().await.unwrap();
    assert_eq!(result.status, RunStatus::Cancelled);
    session.close().await.unwrap();
    client.shutdown().await.unwrap();
    runtime.await.unwrap();
}

#[tokio::test]
async fn rejects_non_monotonic_global_sequence_and_fails_run() {
    let (transport, mut peer, state) = memory_pair("");
    let runtime = tokio::spawn(async move {
        handshake(&mut peer).await;
        let create = peer.request().await;
        peer.send(
            &create.request_id,
            Some("session-sequence"),
            None,
            Event::SessionCreated(SessionEvent::default()),
        );
        let run = peer.request().await;
        peer.send(
            &run.request_id,
            Some("session-sequence"),
            Some("run-sequence"),
            Event::RunStarted(RunStarted::default()),
        );
        let duplicate = peer.sequence;
        peer.send_at(
            duplicate,
            &run.request_id,
            Some("session-sequence"),
            Some("run-sequence"),
            Event::Warning(WarningEvent {
                message: "duplicate sequence".into(),
                code: None,
            }),
        );
    });

    let client = Client::connect(transport).await.unwrap();
    let session = client
        .create_session(SessionOptions::new("o4-test", cwd()))
        .await
        .unwrap();
    let error = session
        .run("test")
        .await
        .unwrap()
        .final_result()
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::ProtocolViolation { reason } if reason.contains("not greater")
    ));
    runtime.await.unwrap();
    assert!(state.aborted.load(Ordering::Acquire));
}

#[tokio::test]
async fn malformed_output_fails_all_pending_handles() {
    let (transport, mut peer, state) = memory_pair("");
    let runtime = tokio::spawn(async move {
        handshake(&mut peer).await;
        let create = peer.request().await;
        peer.send(
            &create.request_id,
            Some("session-malformed"),
            None,
            Event::SessionCreated(SessionEvent::default()),
        );
        let _run = peer.request().await;
        peer.send_raw("this is human output, not JSON");
    });

    let client = Client::connect(transport).await.unwrap();
    let session = client
        .create_session(SessionOptions::new("o4-test", cwd()))
        .await
        .unwrap();
    let error = session
        .run("test")
        .await
        .unwrap()
        .final_result()
        .await
        .unwrap_err();
    assert!(matches!(error, Error::MalformedProtocol { .. }));
    runtime.await.unwrap();
    assert!(state.aborted.load(Ordering::Acquire));
}

#[tokio::test]
async fn process_eof_fails_pending_run_with_bounded_diagnostics() {
    let secret_diagnostic = "provider token should be treated as sensitive";
    let (transport, mut peer, _) = memory_pair(secret_diagnostic);
    let runtime = tokio::spawn(async move {
        handshake(&mut peer).await;
        let create = peer.request().await;
        peer.send(
            &create.request_id,
            Some("session-exit"),
            None,
            Event::SessionCreated(SessionEvent::default()),
        );
        let run = peer.request().await;
        peer.send(
            &run.request_id,
            Some("session-exit"),
            Some("run-exit"),
            Event::RunStarted(RunStarted::default()),
        );
        // Returning drops the event sender and simulates permanent process EOF.
    });

    let client = Client::connect(transport).await.unwrap();
    let session = client
        .create_session(SessionOptions::new("o4-test", cwd()))
        .await
        .unwrap();
    let error = session
        .run("test")
        .await
        .unwrap()
        .final_result()
        .await
        .unwrap_err();
    match error {
        Error::RuntimeExited { diagnostics } => {
            assert_eq!(diagnostics.as_str(), secret_diagnostic);
            assert!(!format!("{diagnostics:?}").contains(secret_diagnostic));
        }
        error => panic!("expected RuntimeExited, received {error:?}"),
    }
    runtime.await.unwrap();
}

#[tokio::test]
async fn validates_runtime_path_and_session_cwd() {
    let missing = cwd().join("definitely-not-an-o4-runtime");
    let error = Client::connect(LocalRuntime::new(&missing))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::InvalidRuntimePath { path, .. } if path == missing
    ));

    let (transport, peer, _) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, Arc::clone(&commands)));
    let client = Client::connect(transport).await.unwrap();
    let not_a_directory = cwd().join("Cargo.toml");
    let error = client
        .create_session(SessionOptions::new("o4-test", &not_a_directory))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::InvalidWorkingDirectory { path, .. } if path == not_a_directory
    ));
    client.shutdown().await.unwrap();
    runtime.await.unwrap();
    assert_eq!(commands.lock().unwrap().as_slice(), &[Command::Shutdown]);
}

#[tokio::test]
async fn policy_and_mcp_use_exact_wire_fields_and_debug_redacts_values() {
    let (transport, peer, state) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, Arc::clone(&commands)));
    let client = Client::connect(transport).await.unwrap();
    let stdio = McpServerConfig::Stdio {
        name: "local-tools".into(),
        command: "tool-server".into(),
        args: vec!["--token=argument-secret".into()],
        env: BTreeMap::from([("API_TOKEN".into(), "environment-secret".into())]),
        cwd: None,
    };
    let sse = McpServerConfig::Sse {
        name: "remote-tools".into(),
        url: "https://credential@example.invalid/sse".into(),
        headers: BTreeMap::from([("Authorization".into(), "Bearer header-secret".into())]),
    };
    for debug in [format!("{stdio:?}"), format!("{sse:?}")] {
        assert!(!debug.contains("argument-secret"));
        assert!(!debug.contains("environment-secret"));
        assert!(!debug.contains("header-secret"));
        assert!(!debug.contains("credential@example"));
    }
    let policy = ExecutionPolicy {
        allowed_tools: vec!["read".into()],
        denied_tools: vec!["shell".into()],
        sandbox_mode: SandboxMode::WorkspaceWrite,
        sandbox_config: Some(json!({"writable_roots": ["workspace"]})),
        allowed_domains: vec!["docs.example.com".into()],
    };
    let session = client
        .create_session(
            SessionOptions::new("o4-test", cwd())
                .execution_policy(policy)
                .mcp_server(stdio)
                .mcp_server(sse),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
    client.shutdown().await.unwrap();
    runtime.await.unwrap();

    let lines = state.sent_lines.lock().unwrap();
    assert!(lines.iter().all(|line| !line.contains('\n')));
    let create: Value = serde_json::from_str(&lines[1]).unwrap();
    let command = &create["command"];
    assert_eq!(command["type"], "session_create");
    assert_eq!(
        command["execution_policy"]["allowed_tools"],
        json!(["read"])
    );
    assert_eq!(
        command["execution_policy"]["denied_tools"],
        json!(["shell"])
    );
    assert_eq!(
        command["execution_policy"]["sandbox_mode"],
        "workspace_write"
    );
    assert_eq!(
        command["execution_policy"]["allowed_domains"],
        json!(["docs.example.com"])
    );
    assert_eq!(command["mcp_servers"][0]["transport"], "stdio");
    assert_eq!(
        command["mcp_servers"][0]["env"]["API_TOKEN"],
        "environment-secret"
    );
    assert_eq!(command["mcp_servers"][1]["transport"], "sse");
    assert_eq!(
        command["mcp_servers"][1]["headers"]["Authorization"],
        "Bearer header-secret"
    );
}

#[tokio::test]
async fn dropping_session_sends_no_close_or_delete_command() {
    let (transport, peer, _) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, Arc::clone(&commands)));
    let client = Client::connect(transport).await.unwrap();
    let session = client
        .create_session(SessionOptions::new("o4-test", cwd()).session_id("drop-is-not-delete"))
        .await
        .unwrap();
    drop(session);
    tokio::task::yield_now().await;
    client.shutdown().await.unwrap();
    runtime.await.unwrap();

    let commands = commands.lock().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(matches!(commands[0], Command::SessionCreate { .. }));
    assert!(matches!(commands[1], Command::Shutdown));
}

#[tokio::test]
async fn abstract_transport_connects_without_process_or_pty_assumptions() {
    let (transport, peer, state) = memory_pair("");
    let commands = Arc::new(Mutex::new(Vec::new()));
    let runtime = tokio::spawn(auto_runtime(peer, commands));
    let client = Client::connect(transport).await.unwrap();
    client.shutdown().await.unwrap();
    runtime.await.unwrap();

    // The fake implements only send/receive/close/abort. A successful complete
    // lifecycle proves the public transport contract needs neither a process
    // handle nor terminal/PTY operations.
    assert!(state.closed.load(Ordering::Acquire));
    assert!(!state.aborted.load(Ordering::Acquire));
}

/// `compatibility.toml` documents the pin; `DEFAULT_RUNTIME_VERSION_REQUIREMENT`
/// is what the handshake actually enforces. Nothing generates one from the
/// other, so they can drift silently -- and did: the file kept saying
/// `<0.1.0` while the runtime shipped 0.2.x, which only surfaced as an e2e
/// handshake rejection. Compare them directly, and require that the verified
/// `runtime_version` satisfies the range it is pinned alongside.
#[test]
fn compatibility_pin_matches_compiled_default() {
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility.toml"),
    )
    .expect("read compatibility.toml");

    let field = |key: &str| -> String {
        manifest
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key} = ")))
            .unwrap_or_else(|| panic!("{key} present in compatibility.toml"))
            .trim()
            .trim_matches('"')
            .to_owned()
    };

    let pinned_requirement = field("runtime_version_requirement");
    assert_eq!(
        pinned_requirement, DEFAULT_RUNTIME_VERSION_REQUIREMENT,
        "compatibility.toml and the compiled default must state the same range"
    );

    let verified: semver::Version = field("runtime_version")
        .parse()
        .expect("runtime_version is semver");
    let requirement: semver::VersionReq = pinned_requirement
        .parse()
        .expect("runtime_version_requirement is a semver range");
    assert!(
        requirement.matches(&verified),
        "verified runtime {verified} must satisfy the pinned range {pinned_requirement}"
    );
}
