//! Ask a person before the agent changes anything.
//!
//! ```text
//! cargo run -p o4-agent-sdk --example human_approval -- \
//!     <path-to-o4-agent-runtime> <model> [prompt...]
//! ```
//!
//! The agent works in the current directory with only the file tools, and
//! may edit files there, but each edit waits for an answer typed here: `y`
//! allows it, `a` allows that tool for the rest of the session, anything
//! else refuses. In a terminal, lines entered before a question appears are
//! dropped, so an early `y` cannot answer it (text typed without pressing
//! Enter is not; that stays in the terminal until Enter). If input ends,
//! every request is refused.

use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::io::{BufRead, IsTerminal, Write};

use o4_agent_sdk::{
    ApprovalDecision, ApprovalOutcome, ApprovalRequest, ApprovalStream, Client, ClientOptions,
    Event, ExecutionPolicy, LocalRuntime, RunResult, RunStatus, SandboxMode, Session,
    SessionOptions,
};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: human_approval <path-to-o4-agent-runtime> <model> [prompt...]";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let prompt = args.collect::<Vec<_>>().join(" ");
    let prompt = if prompt.is_empty() {
        "Create a file named notes.txt containing a short greeting.".to_owned()
    } else {
        prompt
    };

    let client = Client::connect_with_options(
        LocalRuntime::new(runtime),
        ClientOptions::default().require_capability("interactive_approvals"),
    )
    .await?;
    let outcome = supervise(&client, &model, prompt).await;
    let shutdown = client.shutdown().await;
    let result = outcome?;
    shutdown?;

    if result.status != RunStatus::Completed {
        return Err(format!("the run ended {:?}: {}", result.status, result.output).into());
    }
    println!("{}", result.output);
    Ok(())
}

async fn supervise(
    client: &Client,
    model: &str,
    prompt: String,
) -> Result<RunResult, Box<dyn Error>> {
    // Take the request stream before creating a session that asks. There is
    // one stream per connection.
    let mut approvals = client.approvals().await?;
    let session = client
        .create_session(
            SessionOptions::new(model, env::current_dir()?).execution_policy(ExecutionPolicy {
                // Only these tools. Leaving out `agent` (sub-agents) and
                // `exec` (code mode) matters: in runtimes 0.3.21 to 0.3.38 their
                // tool calls do not wait for an answer.
                allowed_tools: ["read", "ls", "glob", "grep", "write", "edit"]
                    .map(String::from)
                    .to_vec(),
                // Allows editing files inside the workspace...
                sandbox_mode: SandboxMode::WorkspaceWrite,
                // ...and makes every change wait for an answer.
                interactive_approvals: true,
                ..ExecutionPolicy::default()
            }),
        )
        .await?;
    let outcome = answer_while_running(&session, &mut approvals, prompt).await;
    let closed = session.close().await;
    let result = outcome?;
    closed?;
    Ok(result)
}

async fn answer_while_running(
    session: &Session,
    approvals: &mut ApprovalStream,
    prompt: String,
) -> Result<RunResult, Box<dyn Error>> {
    let mut run = session.run(prompt).await?;
    // `None` once input has ended.
    let mut input = Some(typed_lines());
    // Requests asked about one at a time, oldest first.
    let mut waiting = VecDeque::new();
    // Requests and run events are read in one loop: a run waiting for an
    // answer only continues once someone answers.
    loop {
        tokio::select! {
            event = run.next_event() => {
                let Some(envelope) = event? else { break };
                match &envelope.event {
                    Event::ToolCompleted(tool) => {
                        let name = tool.tool_call["name"].as_str().unwrap_or("tool");
                        let failed = tool.tool_call["is_error"] == true;
                        let how = if failed { "failed or refused" } else { "done" };
                        // `{:?}` escapes control characters in a tool name.
                        println!("[{name:?}] {how}");
                    }
                    Event::RunCompleted(_) => break,
                    _ => {}
                }
            }
            Some(request) = approvals.next_request() => {
                if input.is_none() {
                    refuse(&request).await?;
                } else {
                    waiting.push_back(request);
                    if waiting.len() == 1 {
                        discard_typed_ahead(&mut input);
                        ask(&waiting[0]);
                    }
                }
            }
            line = next_line(&mut input), if input.is_some() && !waiting.is_empty() => {
                let Some(line) = line else {
                    println!("\ninput ended: refusing every request");
                    input = None;
                    for request in waiting.drain(..) {
                        refuse(&request).await?;
                    }
                    continue;
                };
                let request = waiting.pop_front().expect("a request is waiting");
                match decision(&line, &request) {
                    Some(decision) => answer(&request, decision).await?,
                    None => {
                        println!("`a` is not offered for this request");
                        waiting.push_front(request);
                    }
                }
                if let Some(next) = waiting.front() {
                    discard_typed_ahead(&mut input);
                    ask(next);
                }
            }
        }
    }
    Ok(run.final_result().await?)
}

fn ask(request: &ApprovalRequest) {
    let always = if request.always_allow_available() {
        ", a = always for this tool"
    } else {
        ""
    };
    // The tool name and summary come from the model and the tool's provider:
    // `{:?}` prints them quoted, with control characters escaped.
    print!(
        "\nAllow {:?} on {:?}? (y = yes{always}, anything else = no): ",
        request.tool(),
        request.summary()
    );
    let _ = std::io::stdout().flush();
}

/// `None` asks again: the person chose something this request does not offer.
fn decision(line: &str, request: &ApprovalRequest) -> Option<ApprovalDecision> {
    match line.trim() {
        "y" | "yes" => Some(ApprovalDecision::Approve),
        "a" | "always" if request.always_allow_available() => Some(ApprovalDecision::AlwaysAllow),
        "a" | "always" => None,
        _ => Some(ApprovalDecision::Deny),
    }
}

async fn refuse(request: &ApprovalRequest) -> o4_agent_sdk::Result<()> {
    println!("refused {:?} on {:?}", request.tool(), request.summary());
    answer(request, ApprovalDecision::Deny).await
}

async fn answer(request: &ApprovalRequest, decision: ApprovalDecision) -> o4_agent_sdk::Result<()> {
    match request.respond(decision).await? {
        ApprovalOutcome::Recorded => {}
        ApprovalOutcome::AlreadyResolved => println!("(already answered elsewhere)"),
    }
    Ok(())
}

/// In a terminal, drop lines typed before the question: an early `y` must not
/// approve a request the person has not seen. Piped input is kept, since a
/// script writes its answers in advance.
fn discard_typed_ahead(input: &mut Option<mpsc::UnboundedReceiver<String>>) {
    if let Some(lines) = input.as_mut().filter(|_| std::io::stdin().is_terminal()) {
        while lines.try_recv().is_ok() {}
    }
}

async fn next_line(input: &mut Option<mpsc::UnboundedReceiver<String>>) -> Option<String> {
    input.as_mut()?.recv().await
}

/// Lines typed on standard input. A plain thread does the blocking reads,
/// because a read on Tokio's own stdin cannot be interrupted and can keep the
/// program from exiting.
fn typed_lines() -> mpsc::UnboundedReceiver<String> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}
