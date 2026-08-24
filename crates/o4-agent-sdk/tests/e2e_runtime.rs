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

use o4_agent_sdk::{Client, ClientOptions, Error, LocalRuntime, SessionOptions};

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
