//! End-to-end test: the SDK client drives the real `o4-agent-runtime` binary
//! through `LocalRuntime`.
//!
//! These tests are provider-independent: they exercise the handshake, version
//! and capability negotiation, session lifecycle, and error handling against
//! the actual runtime process. They do not contact an LLM provider.
//!
//! The runtime binary path is resolved from `O4_AGENT_RUNTIME_BIN`, falling
//! back to the shared cargo target dir. Tests skip (rather than fail) when no
//! binary is available so CI environments without the runtime can still run
//! the pure-SDK suites.

use std::path::PathBuf;

use o4_agent_sdk::{
    Client, ClientOptions, Error, Event, ExecutionPolicy, LocalRuntime, ResumeOptions, RunStatus,
    SandboxMode, SessionOptions,
};

/// Locate the runtime binary, or return None to skip.
fn runtime_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("O4_AGENT_RUNTIME_BIN") {
        let path = PathBuf::from(path);
        return path.exists().then_some(path);
    }
    let home = std::env::var("HOME").ok()?;
    let candidate = PathBuf::from(home).join(".cache/cargo-target/debug/o4-agent-runtime");
    candidate.exists().then_some(candidate)
}

async fn connect() -> Option<Client> {
    let binary = runtime_binary()?;
    let source = LocalRuntime::new(binary);
    Client::connect_with_options(
        source,
        ClientOptions::default().client_identity("o4-agent-sdk-e2e", env!("CARGO_PKG_VERSION")),
    )
    .await
    .ok()
}

#[tokio::test]
async fn handshake_negotiates_protocol_and_capabilities() {
    let Some(client) = connect().await else {
        eprintln!("skipping: o4-agent-runtime binary not found");
        return;
    };

    let info = client.runtime_info();
    assert_eq!(info.protocol_version, 1);
    for cap in ["session.resume", "run.cancel", "run.steer", "mcp"] {
        assert!(
            info.capabilities.iter().any(|c| c == cap),
            "runtime missing capability {cap}"
        );
    }

    client.shutdown().await.expect("clean shutdown");
}

#[tokio::test]
async fn create_and_close_session_against_real_runtime() {
    let Some(client) = connect().await else {
        eprintln!("skipping: o4-agent-runtime binary not found");
        return;
    };

    let cwd = std::env::temp_dir();
    let session = client
        .create_session(
            SessionOptions::new("claude-sonnet-4-6", &cwd).system_prompt("You are a test harness."),
        )
        .await
        .expect("session create should succeed against a valid cwd");

    assert!(!session.id().is_empty());

    session.close().await.expect("session close");
    client.shutdown().await.expect("clean shutdown");
}

#[tokio::test]
async fn durable_context_survives_two_distinct_runtime_processes() {
    let Some(binary) = runtime_binary() else {
        eprintln!("skipping: o4-agent-runtime binary not found");
        return;
    };
    let data_dir = tempfile::tempdir().expect("temporary runtime data directory");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let session_id = "sdk-cold-restart-smoke";
    let policy = ExecutionPolicy {
        allowed_tools: vec!["write".into()],
        denied_tools: Vec::new(),
        sandbox_mode: SandboxMode::WorkspaceWrite,
        sandbox_config: None,
        allowed_domains: Vec::new(),
    };

    let first_client = Client::connect(LocalRuntime::new(&binary).data_dir(data_dir.path()))
        .await
        .expect("connect first runtime process");
    let first_session = first_client
        .create_session(
            SessionOptions::new("open4rena-smoke:open4rena-usage-smoke", workspace.path())
                .session_id(session_id)
                .execution_policy(policy),
        )
        .await
        .expect("create durable session");
    let mut first_run = first_session
        .run("O4_EVAL_HARNESS_WRITE_SMOKE")
        .await
        .expect("start first run");
    let mut first_runtime_id = None;
    let mut successful_write = false;
    while let Some(envelope) = first_run.next_event().await.expect("first run event") {
        first_runtime_id.get_or_insert_with(|| envelope.runtime_id.clone());
        if let Event::ToolCompleted(tool) = envelope.event {
            successful_write |=
                tool.tool_call["name"] == "write" && tool.tool_call["is_error"] == false;
        }
    }
    let first_result = first_run.final_result().await.expect("first run result");
    assert_eq!(first_result.status, RunStatus::Completed);
    assert!(
        successful_write,
        "the production write tool must complete successfully"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("eval-harness-smoke.txt"))
            .expect("smoke output file"),
        "o4 eval harness smoke ok\n"
    );
    first_session.close().await.expect("close first session");
    first_client
        .shutdown()
        .await
        .expect("stop first runtime process");

    let second_client = Client::connect(LocalRuntime::new(&binary).data_dir(data_dir.path()))
        .await
        .expect("connect second runtime process");
    let second_session = second_client
        .resume_session(session_id, ResumeOptions::default())
        .await
        .expect("resume durable session");
    let mut second_run = second_session
        .run("Continue from the persisted context.")
        .await
        .expect("start resumed run");
    let mut second_runtime_id = None;
    while let Some(envelope) = second_run.next_event().await.expect("resumed run event") {
        second_runtime_id.get_or_insert_with(|| envelope.runtime_id.clone());
    }
    let second_result = second_run.final_result().await.expect("resumed run result");

    assert_ne!(
        first_runtime_id.expect("first runtime id"),
        second_runtime_id.expect("second runtime id"),
        "cold resume must use a distinct runtime process"
    );
    assert_eq!(second_result.status, RunStatus::Completed);
    assert_eq!(second_result.output, "O4_EVAL_HARNESS_WRITE_SMOKE_OK");

    second_session.close().await.expect("close resumed session");
    second_client
        .shutdown()
        .await
        .expect("stop second runtime process");
}

#[tokio::test]
async fn invalid_cwd_is_rejected_with_typed_error() {
    let Some(client) = connect().await else {
        eprintln!("skipping: o4-agent-runtime binary not found");
        return;
    };

    let result = client
        .create_session(SessionOptions::new(
            "claude-sonnet-4-6",
            "/nonexistent-dir-o4-e2e-xyz",
        ))
        .await;

    match result {
        Err(Error::InvalidWorkingDirectory { .. })
        | Err(Error::ProtocolViolation { .. })
        | Err(Error::Runtime { .. }) => {}
        other => panic!("expected a typed cwd/protocol error, got {other:?}"),
    }

    client.shutdown().await.expect("clean shutdown");
}
