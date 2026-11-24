//! `huxley mcp`: the MCP server over stdio, for clients that start servers
//! as local processes. It relays newline-delimited JSON-RPC between stdin
//! and stdout and a Huxley server's `/mcp`, with your API token.

use std::time::Duration;

use anyhow::Context as _;
use clap::Args;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

#[derive(Debug, Args)]
pub struct McpArgs {
    /// The Huxley server, e.g. `https://huxley.example.com`.
    #[arg(long, env = "HUXLEY_SERVER")]
    server: String,
    /// An API token.
    #[arg(long, env = "HUXLEY_TOKEN", hide_env_values = true)]
    token: String,
}

pub async fn run(args: McpArgs) -> anyhow::Result<()> {
    huxley_runners::install_crypto_provider();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let url = format!("{}/mcp", args.server.trim_end_matches('/'));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let message: serde_json::Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(e) => {
                let error = serde_json::json!({ "jsonrpc": "2.0", "id": null,
                    "error": { "code": -32700, "message": format!("not JSON: {e}") } });
                stdout.write_all(format!("{error}\n").as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };
        let id = message.get("id").cloned();
        let response = http
            .post(&url)
            .bearer_auth(&args.token)
            .header("accept", "application/json, text/event-stream")
            .json(&message)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"));
        let reply = match response {
            Ok(r) if r.status() == reqwest::StatusCode::ACCEPTED => None,
            Ok(r) if r.status().is_success() => Some(r.text().await?),
            Ok(r) => id.map(|id| {
                serde_json::json!({ "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32000, "message": format!("Huxley answered {}", r.status()) } })
                .to_string()
            }),
            Err(e) => id.map(|id| {
                serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": format!("{e:#}") } })
                    .to_string()
            }),
        };
        if let Some(reply) = reply {
            // One message per line.
            stdout
                .write_all(reply.replace('\n', " ").as_bytes())
                .await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
