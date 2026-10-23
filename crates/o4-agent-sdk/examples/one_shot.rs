use std::env;
use std::path::PathBuf;

use o4_agent_sdk::{Client, Event, LocalRuntime, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: one_shot <path-to-o4-agent-runtime> <model> [prompt]";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let prompt = args
        .next()
        .unwrap_or_else(|| "Summarize the files in this workspace.".into());
    let cwd = env::current_dir()?;

    let client = Client::connect(LocalRuntime::new(runtime)).await?;
    let outcome = run_once(&client, model, cwd, prompt).await;
    // Stop the runtime whether or not the run worked.
    let shutdown = client.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}

async fn run_once(
    client: &Client,
    model: String,
    cwd: PathBuf,
    prompt: String,
) -> o4_agent_sdk::Result<()> {
    let session = client
        .create_session(SessionOptions::new(model, cwd))
        .await?;
    let outcome = async {
        let mut run = session.run(prompt).await?;
        while let Some(envelope) = run.next_event().await? {
            match &envelope.event {
                Event::MessageUpdated(message) | Event::MessageCompleted(message) => {
                    println!("{}", message.message);
                }
                Event::Warning(warning) => eprintln!("warning: {}", warning.message),
                Event::RunCompleted(_) => break,
                _ => {}
            }
        }
        let result = run.final_result().await?;
        println!("final output: {}", result.output);
        Ok(())
    }
    .await;
    // Closing unloads the session; it does not delete durable state.
    let closed = session.close().await;
    outcome?;
    closed
}
