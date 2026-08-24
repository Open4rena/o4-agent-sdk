use std::env;

use o4_agent_sdk::{Client, Event, LocalRuntime, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let runtime = args
        .next()
        .ok_or("usage: one_shot <path-to-o4-agent-runtime> [prompt]")?;
    let prompt = args
        .next()
        .unwrap_or_else(|| "Summarize the files in this workspace.".into());
    let cwd = env::current_dir()?;

    let client = Client::connect(LocalRuntime::new(runtime)).await?;
    let session = client
        .create_session(SessionOptions::new("o4", cwd))
        .await?;
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

    // Closing unloads the session; it does not delete durable state.
    session.close().await?;
    client.shutdown().await?;
    Ok(())
}
