use std::env;

use o4_agent_sdk::{Client, LocalRuntime, ResumeOptions, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = env::args()
        .nth(1)
        .ok_or("usage: persistent <path-to-o4-agent-runtime>")?;
    let cwd = env::current_dir()?;

    let client = Client::connect(LocalRuntime::new(runtime)).await?;
    let session = client
        .create_session(
            SessionOptions::new("o4", &cwd)
                .session_id("sdk-example-session")
                .system_prompt("Remember prior turns and answer concisely."),
        )
        .await?;

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

    let durable_id = session.id().to_owned();
    session.close().await?;

    // Resume the same durable state. `cwd` and request context may be updated.
    let resumed = client
        .resume_session(durable_id, ResumeOptions::default().cwd(&cwd))
        .await?;
    let third = resumed
        .run("Repeat it once more.")
        .await?
        .final_result()
        .await?;
    println!("resumed turn: {}", third.output);

    resumed.close().await?;
    client.shutdown().await?;
    Ok(())
}
