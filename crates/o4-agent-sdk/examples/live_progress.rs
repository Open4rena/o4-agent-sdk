//! Show a run's progress as it happens, and cancel it by pressing Enter.
//!
//! ```text
//! cargo run -p o4-agent-sdk --example live_progress -- \
//!     <path-to-o4-agent-runtime> <model> [prompt...]
//! ```
//!
//! The agent works in the current directory under the runtime's default
//! read-only policy. Pressing Enter asks the runtime to cancel the run; the
//! example keeps reading until the runtime says how the run ended.
//!
//! Ctrl-C is not a graceful stop: the terminal sends it to this program and
//! the runtime together, and both stop at once without cancelling the run.

use std::env;
use std::error::Error;
use std::io::{BufRead, Write};

use o4_agent_sdk::{Client, Event, LocalRuntime, RunStatus, Session, SessionOptions};
use serde_json::Value;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: live_progress <path-to-o4-agent-runtime> <model> [prompt...]";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let prompt = args.collect::<Vec<_>>().join(" ");
    let prompt = if prompt.is_empty() {
        "Describe the files in this directory.".to_owned()
    } else {
        prompt
    };

    let client = Client::connect(LocalRuntime::new(runtime)).await?;
    let outcome = async {
        let session = client
            .create_session(SessionOptions::new(model, env::current_dir()?))
            .await?;
        let watched = watch(&session, prompt).await;
        let closed = session.close().await;
        let status = watched?;
        closed?;
        Ok::<_, Box<dyn Error>>(status)
    }
    .await;
    let shutdown = client.shutdown().await;
    let status = outcome?;
    shutdown?;
    println!("\nrun ended: {status:?}");
    Ok(())
}

async fn watch(session: &Session, prompt: String) -> Result<RunStatus, Box<dyn Error>> {
    let mut run = session.run(prompt).await?;
    eprintln!("(press Enter to cancel)");
    let mut enter = enter_presses();
    // Text of the current message already on screen.
    let mut shown = String::new();
    loop {
        tokio::select! {
            event = run.next_event() => {
                let Some(envelope) = event? else { break };
                match &envelope.event {
                    Event::MessageStarted(_) => {
                        if !shown.is_empty() {
                            println!();
                        }
                        shown.clear();
                    }
                    Event::MessageUpdated(message) | Event::MessageCompleted(message) => {
                        if message.message["role"] == "assistant" {
                            show(&mut shown, &text_of(&message.message));
                        }
                    }
                    Event::ToolStarted(tool) => {
                        println!("\n[tool] {} started", tool.tool_call["name"]);
                    }
                    Event::ToolCompleted(tool) => {
                        let failed = tool.tool_call["is_error"] == true;
                        let how = if failed { "failed" } else { "finished" };
                        println!("[tool] {} {how}", tool.tool_call["name"]);
                    }
                    Event::Warning(warning) => eprintln!("\n[warning] {}", warning.message),
                    Event::RunCompleted(_) => break,
                    // Lifecycle events, and event types added by newer runtimes.
                    _ => {}
                }
            }
            Some(()) = enter.recv() => {
                eprintln!("\n[cancelling] waiting for the run to end");
                match run.cancel().await {
                    Ok(()) => {}
                    // The run finished before the request reached it.
                    Err(o4_agent_sdk::Error::RunFinished { .. }) => {}
                    Err(o4_agent_sdk::Error::Runtime { code, .. }) if code == "run_not_found" => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    // A cancel request is not proof the run stopped: the final result says
    // how it ended, which may still be `Completed`.
    Ok(run.final_result().await?.status)
}

/// Snapshots carry the whole message so far, not just the new part: print
/// what was added, or the whole text again if earlier text changed.
fn show(shown: &mut String, text: &str) {
    // Model text is untrusted: drop control characters, which could move the
    // cursor or rewrite the terminal, but keep line breaks and tabs.
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let text = text.as_str();
    match text.strip_prefix(shown.as_str()) {
        Some(added) => print!("{added}"),
        None => print!("\n{text}"),
    }
    let _ = std::io::stdout().flush();
    *shown = text.to_owned();
}

/// The text blocks of a message snapshot. The snapshot format belongs to the
/// runtime; read it defensively.
fn text_of(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

/// One notice per line typed on standard input. A plain thread does the
/// blocking reads, because a read on Tokio's own stdin cannot be interrupted
/// and can keep the program from exiting.
fn enter_presses() -> mpsc::UnboundedReceiver<()> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if line.is_err() || sender.send(()).is_err() {
                break;
            }
        }
    });
    receiver
}
