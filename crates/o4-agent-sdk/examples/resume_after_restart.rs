//! Continue a conversation after the application restarts.
//!
//! ```text
//! cargo run -p o4-agent-sdk --example resume_after_restart -- \
//!     <path-to-o4-agent-runtime> <model> <state-dir> [prompt...]
//! ```
//!
//! The first run starts a conversation in the current directory and saves its
//! ID in `<state-dir>`. Each later run starts a new runtime process, resumes
//! that conversation and adds a turn. The runtime saves the conversation
//! itself under `<state-dir>/runtime`.
//!
//! A conversation stays tied to the directory it started in, so run the
//! example from that directory each time. Delete `<state-dir>` to start over.

use std::env;
use std::error::Error;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use o4_agent_sdk::{Client, LocalRuntime, ResumeOptions, RunStatus, Session, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let usage =
        "usage: resume_after_restart <path-to-o4-agent-runtime> <model> <state-dir> [prompt...]";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let state = PathBuf::from(args.next().ok_or(usage)?);
    let prompt = args.collect::<Vec<_>>().join(" ");
    fs::create_dir_all(&state)?;

    // Keep the runtime's saved sessions with the application's own state:
    // they must survive restarts, and they stay out of the default `~/.o4`.
    let client =
        Client::connect(LocalRuntime::new(runtime).data_dir(state.join("runtime"))).await?;
    let outcome = take_turn(&client, &model, &state.join("session-id"), prompt).await;
    let shutdown = client.shutdown().await;
    let output = outcome?;
    shutdown?;
    println!("{output}");
    Ok(())
}

async fn take_turn(
    client: &Client,
    model: &str,
    id_file: &Path,
    prompt: String,
) -> Result<String, Box<dyn Error>> {
    let workspace = env::current_dir()?;
    let (session, prompt) = match fs::read_to_string(id_file) {
        Ok(id) => {
            let id = id.trim();
            println!("resuming conversation {id}");
            // `cwd` asserts this is the directory the conversation started in;
            // the runtime refuses to resume it from anywhere else.
            let session = client
                .resume_session(id, ResumeOptions::default().cwd(&workspace))
                .await?;
            (
                session,
                or_default(prompt, "What did I ask you to remember?"),
            )
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let session = client
                .create_session(
                    SessionOptions::new(model, &workspace)
                        .system_prompt("Remember what the user tells you and answer concisely."),
                )
                .await?;
            // Save the ID before the first turn: the session is resumable even
            // if that turn fails.
            fs::write(id_file, session.id())?;
            println!("started conversation {}", session.id());
            let prompt = or_default(prompt, "Remember this: the project's codename is Kestrel.");
            (session, prompt)
        }
        Err(error) => return Err(error.into()),
    };

    let outcome = run(&session, prompt).await;
    // Closing unloads the session; the conversation stays saved.
    let closed = session.close().await;
    let output = outcome?;
    closed?;
    Ok(output)
}

async fn run(session: &Session, prompt: String) -> Result<String, Box<dyn Error>> {
    let result = session.run(prompt).await?.final_result().await?;
    if result.status != RunStatus::Completed {
        return Err(format!("the turn ended {:?}: {}", result.status, result.output).into());
    }
    Ok(result.output)
}

fn or_default(prompt: String, default: &str) -> String {
    if prompt.is_empty() {
        default.to_owned()
    } else {
        prompt
    }
}
