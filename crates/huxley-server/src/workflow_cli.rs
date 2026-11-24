//! `huxley workflows`: workflows as code (design doc §6.7). Validate HWF
//! files locally, and pull, diff, push and run them against a Huxley
//! server with an API token.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use serde_json::{Value as Json, json};

#[derive(Debug, Args)]
pub struct Connection {
    /// The Huxley server, e.g. `https://huxley.example.com`.
    #[arg(long, env = "HUXLEY_SERVER")]
    server: String,
    /// An API token (Settings → Profile → API tokens).
    #[arg(long, env = "HUXLEY_TOKEN", hide_env_values = true)]
    token: String,
    /// The organization: its id or exact name.
    #[arg(long, env = "HUXLEY_ORG")]
    org: String,
}

#[derive(Debug, Subcommand)]
pub enum WorkflowCommand {
    /// Check workflow files (YAML or JSON), without a server. Exits 1 if any
    /// has problems.
    Validate {
        /// Files, or folders of .yaml/.yml/.json files.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Write the organization's workflows (their drafts) as YAML files.
    Pull {
        #[command(flatten)]
        server: Connection,
        /// Where to write them.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Only these workflow keys (default: all).
        keys: Vec<String>,
    },
    /// Show how local files differ from the server's drafts. Exits 1 if
    /// any differ.
    Diff {
        #[command(flatten)]
        server: Connection,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Save files as the server's drafts (creating workflows that don't
    /// exist yet), and optionally publish them.
    Push {
        #[command(flatten)]
        server: Connection,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Publish each one after saving it (needs `workflow:publish`).
        #[arg(long)]
        publish: bool,
        /// Release notes for the published versions.
        #[arg(long)]
        notes: Option<String>,
    },
    /// Run a workflow and wait for it to finish; prints its outputs.
    Run {
        #[command(flatten)]
        server: Connection,
        key: String,
        /// An input, as `name=value` (the value is JSON if it parses).
        #[arg(long = "input", short = 'i')]
        inputs: Vec<String>,
        /// A JSON file of inputs.
        #[arg(long)]
        inputs_file: Option<PathBuf>,
        /// Test-run the draft instead of the published version.
        #[arg(long)]
        draft: bool,
        /// Start it and print its id without waiting.
        #[arg(long)]
        no_wait: bool,
    },
}

pub async fn run(command: WorkflowCommand) -> anyhow::Result<()> {
    match command {
        WorkflowCommand::Validate { paths } => validate(&paths),
        WorkflowCommand::Pull { server, dir, keys } => {
            pull(&Client::new(&server).await?, &dir, &keys).await
        }
        WorkflowCommand::Diff { server, paths } => diff(&Client::new(&server).await?, &paths).await,
        WorkflowCommand::Push {
            server,
            paths,
            publish,
            notes,
        } => {
            push(
                &Client::new(&server).await?,
                &paths,
                publish,
                notes.as_deref(),
            )
            .await
        }
        WorkflowCommand::Run {
            server,
            key,
            inputs,
            inputs_file,
            draft,
            no_wait,
        } => {
            let inputs = run_inputs(&inputs, inputs_file.as_deref())?;
            start(&Client::new(&server).await?, &key, inputs, draft, no_wait).await
        }
    }
}

// ---- files ----

/// The workflow files among `paths` (folders expand to their files).
fn files(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| matches!(e, "yaml" | "yml" | "json"))
                })
                .collect();
            found.sort();
            out.extend(found);
        } else {
            out.push(path.clone());
        }
    }
    Ok(out)
}

fn load(path: &Path) -> anyhow::Result<Json> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let document: Json = serde_saphyr::from_str(&text)
        .with_context(|| format!("{} isn't valid YAML or JSON", path.display()))?;
    if !document.is_object() {
        bail!("{} isn't a workflow document", path.display());
    }
    Ok(document)
}

fn key_of(document: &Json, path: &Path) -> anyhow::Result<String> {
    document["metadata"]["key"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("{} has no metadata.key", path.display()))
}

/// Tells editors (via the YAML language server) where HWF's schema is.
const SCHEMA_COMMENT: &str =
    "# yaml-language-server: $schema=https://usehuxley.com/schemas/hwf/v1.json";

/// Keys first in this order, then the rest alphabetically, so pulled files
/// read naturally and diff cleanly.
const KEY_ORDER: &[&str] = &[
    "apiVersion",
    "kind",
    "metadata",
    "spec",
    "key",
    "id",
    "name",
    "description",
    "type",
    "inputs",
    "triggers",
    "if",
    "with",
    "steps",
    "outputs",
];

fn key_rank(key: &str) -> (usize, &str) {
    (
        KEY_ORDER
            .iter()
            .position(|o| *o == key)
            .unwrap_or(KEY_ORDER.len()),
        key,
    )
}

/// Serializes a JSON value with object keys in a stable, readable order
/// (whatever order the map itself keeps).
struct Ordered<'a>(&'a Json);

impl serde::Serialize for Ordered<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap as _, SerializeSeq as _};
        match self.0 {
            Json::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort_by(|a, b| key_rank(a).cmp(&key_rank(b)));
                let mut out = serializer.serialize_map(Some(keys.len()))?;
                for key in keys {
                    out.serialize_entry(key, &Ordered(&map[key]))?;
                }
                out.end()
            }
            Json::Array(items) => {
                let mut out = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    out.serialize_element(&Ordered(item))?;
                }
                out.end()
            }
            other => other.serialize(serializer),
        }
    }
}

/// A workflow document as YAML, in canonical order.
pub fn to_yaml(document: &Json) -> anyhow::Result<String> {
    serde_saphyr::to_string(&Ordered(document)).context("failed to write YAML")
}

fn validate(paths: &[PathBuf]) -> anyhow::Result<()> {
    let expr = huxley_expr::Engine::new();
    let mut problems = 0;
    for path in files(paths)? {
        let diagnostics = match load(&path) {
            Ok(document) => huxley_workflows::diagnose(&expr, &document)
                .into_iter()
                .map(|d| d.to_string())
                .collect(),
            Err(e) => vec![format!("{e:#}")],
        };
        if diagnostics.is_empty() {
            println!("ok    {}", path.display());
        } else {
            for d in &diagnostics {
                println!("error {}: {d}", path.display());
            }
            problems += diagnostics.len();
        }
    }
    if problems > 0 {
        bail!("{problems} problem(s) found");
    }
    Ok(())
}

// ---- the server ----

struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
    org_id: String,
}

impl Client {
    async fn new(server: &Connection) -> anyhow::Result<Self> {
        huxley_runners::install_crypto_provider();
        let mut client = Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()?,
            base: server.server.trim_end_matches('/').to_owned(),
            token: server.token.clone(),
            org_id: String::new(),
        };
        client.org_id = client.resolve_org(&server.org).await?;
        Ok(client)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Json>,
    ) -> anyhow::Result<Json> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("couldn't reach {}", self.base))?;
        let status = response.status();
        let body: Json = response.json().await.unwrap_or(Json::Null);
        if !status.is_success() {
            let detail = body["errors"][0]["message"]
                .as_str()
                .or_else(|| body["detail"].as_str())
                .or_else(|| body["title"].as_str())
                .unwrap_or("request failed");
            bail!("{path}: {status}: {detail}");
        }
        Ok(body)
    }

    async fn get(&self, path: &str) -> anyhow::Result<Json> {
        self.send(reqwest::Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: &Json) -> anyhow::Result<Json> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    async fn resolve_org(&self, org: &str) -> anyhow::Result<String> {
        let orgs = self.get("/api/v1/orgs").await?;
        let found = orgs
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["id"] == org || o["name"] == org)
            .with_context(|| format!("no organization {org:?} that this token can see"))?;
        Ok(found["id"].as_str().unwrap_or_default().to_owned())
    }

    /// The organization's workflows, by key.
    async fn workflows(&self) -> anyhow::Result<BTreeMap<String, Json>> {
        let all = self.get("/api/v1/workflows").await?;
        Ok(all
            .as_array()
            .into_iter()
            .flatten()
            .filter(|w| w["orgId"] == self.org_id.as_str())
            .map(|w| (w["key"].as_str().unwrap_or_default().to_owned(), w.clone()))
            .collect())
    }
}

async fn pull(client: &Client, dir: &Path, keys: &[String]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let workflows = client.workflows().await?;
    for key in keys {
        if !workflows.contains_key(key) {
            bail!("no workflow {key:?} in this organization");
        }
    }
    let mut written = 0;
    for (key, summary) in &workflows {
        if !keys.is_empty() && !keys.contains(key) {
            continue;
        }
        let id = summary["id"].as_str().unwrap_or_default();
        let detail = client.get(&format!("/api/v1/workflows/{id}")).await?;
        let path = dir.join(format!("{key}.yaml"));
        let text = format!("{SCHEMA_COMMENT}\n{}", to_yaml(&detail["draft"])?);
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("pulled {}", path.display());
        written += 1;
    }
    println!("{written} workflow(s) pulled.");
    Ok(())
}

async fn diff(client: &Client, paths: &[PathBuf]) -> anyhow::Result<()> {
    let workflows = client.workflows().await?;
    let mut differ = 0;
    for path in files(paths)? {
        let document = load(&path)?;
        let key = key_of(&document, &path)?;
        let local = to_yaml(&document)?;
        let Some(summary) = workflows.get(&key) else {
            println!("new   {} ({key} isn't on the server yet)", path.display());
            differ += 1;
            continue;
        };
        let id = summary["id"].as_str().unwrap_or_default();
        let remote = to_yaml(&client.get(&format!("/api/v1/workflows/{id}")).await?["draft"])?;
        if remote == local {
            println!("same  {}", path.display());
        } else {
            differ += 1;
            let text = similar::TextDiff::from_lines(&remote, &local);
            print!(
                "{}",
                text.unified_diff()
                    .context_radius(3)
                    .header(&format!("server/{key}"), &path.display().to_string())
            );
        }
    }
    if differ > 0 {
        bail!("{differ} workflow(s) differ from the server");
    }
    Ok(())
}

async fn push(
    client: &Client,
    paths: &[PathBuf],
    publish: bool,
    notes: Option<&str>,
) -> anyhow::Result<()> {
    let workflows = client.workflows().await?;
    for path in files(paths)? {
        let document = load(&path)?;
        let key = key_of(&document, &path)?;
        let saved = match workflows.get(&key) {
            Some(summary) => {
                let id = summary["id"].as_str().unwrap_or_default();
                // The latest revision: the draft may have moved since listing.
                let current = client.get(&format!("/api/v1/workflows/{id}")).await?;
                let body = json!({ "revision": current["revision"], "document": document });
                client
                    .send(
                        reqwest::Method::PUT,
                        &format!("/api/v1/workflows/{id}/draft"),
                        Some(&body),
                    )
                    .await?
            }
            None => {
                client
                    .post(
                        "/api/v1/workflows",
                        &json!({ "orgId": client.org_id, "document": document }),
                    )
                    .await?
            }
        };
        let id = saved["id"].as_str().unwrap_or_default().to_owned();
        let problems: Vec<String> = saved["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                format!(
                    "{}: {}",
                    d["path"].as_str().unwrap_or(""),
                    d["message"].as_str().unwrap_or("")
                )
            })
            .collect();
        println!("saved {} as {key}'s draft", path.display());
        for p in &problems {
            println!("  problem: {p}");
        }
        if publish {
            if !problems.is_empty() {
                bail!("{key} has problems, so it can't be published");
            }
            let published = client
                .post(
                    &format!("/api/v1/workflows/{id}/publish"),
                    &json!({ "notes": notes }),
                )
                .await?;
            println!(
                "published {key} as version {}",
                published["version"].as_i64().unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn run_inputs(pairs: &[String], file: Option<&Path>) -> anyhow::Result<Json> {
    let mut inputs = match file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            serde_json::from_str::<Json>(&text)
                .with_context(|| format!("{} isn't JSON", path.display()))?
        }
        None => json!({}),
    };
    let Some(map) = inputs.as_object_mut() else {
        bail!("the inputs file must hold a JSON object");
    };
    for pair in pairs {
        let (name, value) = pair
            .split_once('=')
            .with_context(|| format!("{pair:?}: inputs are name=value"))?;
        let value = serde_json::from_str(value).unwrap_or_else(|_| Json::String(value.to_owned()));
        map.insert(name.to_owned(), value);
    }
    Ok(inputs)
}

async fn start(
    client: &Client,
    key: &str,
    inputs: Json,
    draft: bool,
    no_wait: bool,
) -> anyhow::Result<()> {
    let workflows = client.workflows().await?;
    let summary = workflows
        .get(key)
        .with_context(|| format!("no workflow {key:?} in this organization"))?;
    let id = summary["id"].as_str().unwrap_or_default();
    let started = client
        .post(
            &format!("/api/v1/workflows/{id}/runs"),
            &json!({ "inputs": inputs, "test": draft }),
        )
        .await?;
    let run_id = started["id"].as_str().unwrap_or_default().to_owned();
    if no_wait {
        println!("{run_id}");
        return Ok(());
    }
    eprintln!("run {run_id} started; waiting for it to finish");
    let mut delay = Duration::from_millis(250);
    loop {
        let run = client.get(&format!("/api/v1/runs/{run_id}")).await?;
        match run["status"].as_str().unwrap_or_default() {
            "succeeded" => {
                println!("{}", serde_json::to_string_pretty(&run["outputs"])?);
                return Ok(());
            }
            status @ ("failed" | "cancelled") => {
                eprintln!("{}", serde_json::to_string_pretty(&run["error"])?);
                bail!("run {run_id} {status}");
            }
            _ => {}
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_order_reads_naturally() {
        let doc = json!({
            "spec": { "steps": [{ "with": { "b": 1, "a": 2 }, "type": "transform", "id": "x" }] },
            "metadata": { "name": "N", "key": "k" },
            "kind": "Workflow",
            "apiVersion": "usehuxley.com/v1"
        });
        let yaml = to_yaml(&doc).unwrap();
        let api = yaml.find("apiVersion").unwrap();
        let kind = yaml.find("kind").unwrap();
        let spec = yaml.find("spec").unwrap();
        assert!(api < kind && kind < spec, "{yaml}");
        assert!(
            yaml.find("id: x").unwrap() < yaml.find("type: transform").unwrap(),
            "{yaml}"
        );
        assert!(
            yaml.find("a: 2").unwrap() < yaml.find("b: 1").unwrap(),
            "{yaml}"
        );
        // It reads back as the same document.
        let back: Json = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn inputs_parse_as_json_when_they_can() {
        let inputs = run_inputs(
            &["name=ada".into(), "count=3".into(), "tags=[\"a\"]".into()],
            None,
        )
        .unwrap();
        assert_eq!(inputs, json!({ "name": "ada", "count": 3, "tags": ["a"] }));
        assert!(run_inputs(&["nonsense".into()], None).is_err());
    }
}
