//! Give a session tools from a local MCP server.
//!
//! ```text
//! cargo run -p o4-agent-sdk --example mcp_tools -- \
//!     <path-to-o4-agent-runtime> <model> [prompt...]
//! ```
//!
//! Uses the small MCP server in `examples/mcp/echo_server.py` (needs
//! `python3`). It offers one tool, `echo`. To use your own server, change
//! `SERVER`, the command and the tool names below.
//!
//! In a sandboxed session the runtime exposes an MCP tool only when the
//! policy names it, as `<server>__<tool>`. Listing tools in `allowed_tools`
//! also means the session gets only the tools listed. The runtime refuses to
//! create the session if the server offers no listed tool.
//!
//! The server runs as an ordinary process outside the session's sandbox. It
//! gets a few basic variables such as `PATH` and `HOME` plus those in `env`,
//! not the application's API keys. Only connect servers you trust.

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::path::Path;

use o4_agent_sdk::{
    Client, ClientOptions, Event, ExecutionPolicy, LocalRuntime, McpServerConfig, RunResult,
    RunStatus, SessionOptions,
};

const SERVER: &str = "examples";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: mcp_tools <path-to-o4-agent-runtime> <model> [prompt...]";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let prompt = args.collect::<Vec<_>>().join(" ");
    let prompt = if prompt.is_empty() {
        "Use the echo tool to say hello.".to_owned()
    } else {
        prompt
    };

    let server = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/mcp/echo_server.py");
    let echo_server = McpServerConfig::Stdio {
        name: SERVER.into(),
        command: "python3".into(),
        args: vec![server.display().to_string()],
        env: BTreeMap::new(),
        cwd: None,
    };
    let policy = ExecutionPolicy {
        // The server's tool, plus the read-only file tools; nothing else.
        allowed_tools: vec![
            format!("{SERVER}__echo"),
            "read".into(),
            "ls".into(),
            "glob".into(),
            "grep".into(),
        ],
        ..ExecutionPolicy::default()
    };

    let client = Client::connect_with_options(
        LocalRuntime::new(runtime),
        ClientOptions::default().require_capability("mcp"),
    )
    .await?;
    let outcome = async {
        // Starting the session starts the server; it stops with the session.
        let session = client
            .create_session(
                SessionOptions::new(model, env::current_dir()?)
                    .execution_policy(policy)
                    .mcp_server(echo_server),
            )
            .await?;
        let ran = run(&session, prompt).await;
        let closed = session.close().await;
        let result = ran?;
        closed?;
        Ok::<_, Box<dyn Error>>(result)
    }
    .await;
    let shutdown = client.shutdown().await;
    let result = outcome?;
    shutdown?;

    if result.status != RunStatus::Completed {
        return Err(format!("the run ended {:?}: {}", result.status, result.output).into());
    }
    println!("{}", result.output);
    Ok(())
}

async fn run(session: &o4_agent_sdk::Session, prompt: String) -> o4_agent_sdk::Result<RunResult> {
    let mut run = session.run(prompt).await?;
    while let Some(envelope) = run.next_event().await? {
        match &envelope.event {
            Event::ToolStarted(tool) => println!("[tool] {} started", tool.tool_call["name"]),
            Event::ToolCompleted(tool) => {
                let failed = tool.tool_call["is_error"] == true;
                let how = if failed { "failed" } else { "finished" };
                println!("[tool] {} {how}", tool.tool_call["name"]);
            }
            Event::RunCompleted(_) => break,
            _ => {}
        }
    }
    run.final_result().await
}
