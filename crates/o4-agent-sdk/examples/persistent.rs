use std::env;
use std::path::Path;

use o4_agent_sdk::{Client, LocalRuntime, ResumeOptions, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: persistent <path-to-o4-agent-runtime> <model>";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let cwd = env::current_dir()?;

    let client = Client::connect(LocalRuntime::new(runtime)).await?;
    let outcome = conversation(&client, &model, &cwd).await;
    // Stop the runtime whether or not the conversation worked.
    let shutdown = client.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}

async fn conversation(client: &Client, model: &str, cwd: &Path) -> o4_agent_sdk::Result<()> {
    // The runtime picks a new session ID each run; a fixed ID would clash
    // with the session saved by the previous run.
    let session = client
        .create_session(
            SessionOptions::new(model, cwd)
                .system_prompt("Remember prior turns and answer concisely."),
        )
        .await?;
    let turns = async {
        let first = session
            .run("My project codename is Kestrel.")
            .await?
            .final_result()
            .await?;
        println!("turn 1: {}", first.output);

        let second = session
            .run("What is the codename?")
            .await?
            .final_result()
            .await?;
        println!("turn 2: {}", second.output);
        Ok(())
    }
    .await;
    // Closing unloads the session; it does not delete durable state.
    let closed = session.close().await;
    turns?;
    closed?;

    // Resume the same durable state. `cwd` must be the directory it started in.
    let resumed = client
        .resume_session(session.id(), ResumeOptions::default().cwd(cwd))
        .await?;
    let third = async {
        resumed
            .run("Repeat it once more.")
            .await?
            .final_result()
            .await
    }
    .await;
    let closed = resumed.close().await;
    println!("resumed turn: {}", third?.output);
    closed
}
