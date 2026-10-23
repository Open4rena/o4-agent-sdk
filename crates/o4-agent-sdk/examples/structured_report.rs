//! Ask for a JSON report that must match a schema, with limits on turns,
//! tokens and time.
//!
//! ```text
//! cargo run -p o4-agent-sdk --example structured_report -- \
//!     <path-to-o4-agent-runtime> <model> [workspace]
//! ```
//!
//! Prints the report, or exits with an error unless the run completed and
//! returned a report matching the schema. The session uses the runtime's
//! default read-only policy, so nothing in the workspace is changed.

use std::env;
use std::error::Error;
use std::path::PathBuf;

use o4_agent_sdk::{
    Client, ClientOptions, LocalRuntime, RunOptions, RunStatus, SessionOptions, Usage,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Report {
    summary: String,
    risks: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let usage = "usage: structured_report <path-to-o4-agent-runtime> <model> [workspace]";
    let mut args = env::args().skip(1);
    let runtime = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let workspace = match args.next() {
        Some(path) => PathBuf::from(path),
        None => env::current_dir()?,
    };

    // Fail at connection, not halfway through the job, if this runtime cannot
    // return schema-checked output.
    let client = Client::connect_with_options(
        LocalRuntime::new(runtime),
        ClientOptions::default().require_capability("structured_output"),
    )
    .await?;
    let outcome = write_report(&client, &model, workspace).await;
    // Stop the runtime whether or not the report worked.
    let shutdown = client.shutdown().await;
    let (report, usage) = outcome?;
    shutdown?;

    println!("summary: {}", report.summary);
    for risk in &report.risks {
        println!("risk: {risk}");
    }
    let cost = match usage.cost_usd {
        Some(cost) => format!("${cost:.4}"),
        // Unknown, not free.
        None => "unknown".into(),
    };
    println!("tokens: {}, cost: {cost}", usage.total_tokens);
    Ok(())
}

async fn write_report(
    client: &Client,
    model: &str,
    workspace: PathBuf,
) -> Result<(Report, Usage), Box<dyn Error>> {
    // Runtime 0.3.21 fails schema runs in sessions whose policy lists tools
    // (`allowed_tools` or `denied_tools`), so this session keeps the default.
    let session = client
        .create_session(SessionOptions::new(model, workspace))
        .await?;
    let options = RunOptions::default()
        .output_schema(json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "risks": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["summary", "risks"],
            "additionalProperties": false
        }))
        .max_turns(8)
        .token_budget(20_000)
        .timeout_secs(120);
    let outcome = async {
        session
            .run_with_options("Summarize this project and list its main risks.", options)
            .await?
            .final_result()
            .await
    }
    .await;
    // Closing unloads the session; it does not delete what was saved.
    let closed = session.close().await;
    let result = outcome?;
    closed?;

    // `Ok` only means the run ended. Check how it ended before trusting it.
    if result.status != RunStatus::Completed {
        return Err(format!(
            "no report: the run ended {:?}: {}",
            result.status, result.output
        )
        .into());
    }
    let value = result
        .structured_output
        .ok_or("no report: the run returned no structured output")?;
    // The runtime checked the schema; the application still checks the type.
    let report = serde_json::from_value(value)?;
    Ok((report, result.usage))
}
