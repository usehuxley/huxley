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
    /// Run fixture tests (`*.test.yaml`): each case test-runs its workflow
    /// file on the server, with mocked steps, and checks the result.
    /// Nothing is saved. Exits 1 if any case fails.
    Test {
        #[command(flatten)]
        server: Connection,
        /// Test files, or folders of them.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// How long one case may take, in seconds.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
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
        WorkflowCommand::Test {
            server,
            paths,
            timeout,
        } => {
            test(
                &Client::new(&server).await?,
                &paths,
                Duration::from_secs(timeout),
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

/// A workflow file, and the folder it says the workflow is in: given for
/// files found in a directory (its subdirectories, from the top; empty:
/// unfiled), none for files named one by one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    path: PathBuf,
    folder: Option<Vec<String>>,
}

/// The workflow files among `paths`. Directories expand to the files in
/// them and their subdirectories (not hidden ones, such as `.git`).
fn found(paths: &[PathBuf]) -> anyhow::Result<Vec<Found>> {
    fn walk(dir: &Path, folder: &[String], out: &mut Vec<Found>) -> anyhow::Result<()> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("failed to read {}", dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if !name.starts_with('.') {
                    let mut inner = folder.to_vec();
                    inner.push(name.to_owned());
                    walk(&path, &inner, out)?;
                }
            } else if is_workflow_file(&path) {
                out.push(Found {
                    path,
                    folder: Some(folder.to_vec()),
                });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &[], &mut out)?;
        } else {
            out.push(Found {
                path: path.clone(),
                folder: None,
            });
        }
    }
    Ok(out)
}

/// The workflow files among `paths`.
fn files(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    Ok(found(paths)?.into_iter().map(|f| f.path).collect())
}

/// A folder's name as a directory name: no slashes, and not hidden.
fn dir_name(name: &str) -> String {
    let name = name.trim().replace(['/', '\\'], "-");
    match name.as_str() {
        "" | "." | ".." => "_".to_owned(),
        n if n.starts_with('.') => format!("_{}", &n[1..]),
        _ => name,
    }
}

fn show_folder(path: &[String]) -> String {
    if path.is_empty() {
        "unfiled".to_owned()
    } else {
        path.join("/")
    }
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".test.yaml") || n.ends_with(".test.yml"))
}

fn is_workflow_file(path: &Path) -> bool {
    !is_test_file(path)
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "yaml" | "yml" | "json"))
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
    "# yaml-language-server: $schema=https://usehuxley.io/schemas/hwf/v1.json";

// HWF's keys in the order people write them: the same rules as the web
// editor's YAML view (apps/web/src/lib/workflows/yaml.ts), so a draft
// reads the same in both. Other keys are sorted.
const TOP: &[&str] = &["apiVersion", "kind", "metadata", "spec", "ui"];
const METADATA: &[&str] = &["key", "name", "description", "tags"];
const SPEC: &[&str] = &[
    "inputs", "triggers", "steps", "outputs", "on_error", "settings",
];
const STEP_FIRST: &[&str] = &["id", "type", "name", "description", "if"];
const STEP_LAST: &[&str] = &[
    "then",
    "else",
    "cases",
    "default",
    "items",
    "as",
    "concurrency",
    "steps",
    "branches",
    "join",
    "catch",
    "finally",
    "retry",
    "timeout",
    "continue_on_error",
];
const TRIGGER_FIRST: &[&str] = &["type", "id"];

/// A JSON value with its object keys in a chosen order.
enum Node {
    Map(Vec<(String, Node)>),
    Seq(Vec<Node>),
    Leaf(Json),
}

impl serde::Serialize for Node {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap as _, SerializeSeq as _};
        match self {
            Self::Map(entries) => {
                let mut out = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    out.serialize_entry(key, value)?;
                }
                out.end()
            }
            Self::Seq(items) => {
                let mut out = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    out.serialize_element(item)?;
                }
                out.end()
            }
            Self::Leaf(value) => value.serialize(serializer),
        }
    }
}

/// Any value, objects' keys sorted.
fn plain(value: &Json) -> Node {
    match value {
        Json::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Node::Map(
                keys.into_iter()
                    .map(|k| (k.clone(), plain(&map[k])))
                    .collect(),
            )
        }
        Json::Array(items) => Node::Seq(items.iter().map(plain).collect()),
        other => Node::Leaf(other.clone()),
    }
}

/// `object`'s keys: `first` (in order), the rest sorted, then `last`; each
/// value converted by `child(key, value)`.
fn ordered(
    object: &serde_json::Map<String, Json>,
    first: &[&str],
    last: &[&str],
    child: &dyn Fn(&str, &Json) -> Node,
) -> Node {
    let mut middle: Vec<&String> = object
        .keys()
        .filter(|k| !first.contains(&k.as_str()) && !last.contains(&k.as_str()))
        .collect();
    middle.sort();
    let keys = first
        .iter()
        .filter(|k| object.contains_key(**k))
        .map(|k| (*k).to_owned())
        .chain(middle.into_iter().cloned())
        .chain(
            last.iter()
                .filter(|k| object.contains_key(**k))
                .map(|k| (*k).to_owned()),
        );
    Node::Map(
        keys.map(|k| {
            let node = child(&k, &object[&k]);
            (k, node)
        })
        .collect(),
    )
}

fn steps(value: &Json) -> Node {
    match value {
        Json::Array(items) => Node::Seq(items.iter().map(step).collect()),
        other => plain(other),
    }
}

fn keyed_blocks(value: &Json, first: &'static [&'static str]) -> Node {
    match value {
        Json::Array(items) => Node::Seq(
            items
                .iter()
                .map(|item| match item {
                    Json::Object(map) => ordered(map, first, &[], &|k, v| {
                        if k == "steps" { steps(v) } else { plain(v) }
                    }),
                    other => plain(other),
                })
                .collect(),
        ),
        other => plain(other),
    }
}

fn step(value: &Json) -> Node {
    let Json::Object(map) = value else {
        return plain(value);
    };
    ordered(map, STEP_FIRST, STEP_LAST, &|key, v| match key {
        "then" | "else" | "default" | "steps" | "catch" | "finally" => steps(v),
        "cases" => keyed_blocks(v, &["when", "steps"]),
        "branches" => keyed_blocks(v, &["id", "steps"]),
        _ => plain(v),
    })
}

/// A workflow document with its keys in the conventional HWF order.
fn order_document(document: &Json) -> Node {
    let Json::Object(map) = document else {
        return plain(document);
    };
    ordered(map, TOP, &[], &|key, v| match (key, v) {
        ("metadata", Json::Object(m)) => ordered(m, METADATA, &[], &|_, v| plain(v)),
        ("spec", Json::Object(spec)) => ordered(spec, SPEC, &[], &|key, v| match key {
            "steps" | "on_error" => steps(v),
            "triggers" => match v {
                Json::Array(items) => Node::Seq(
                    items
                        .iter()
                        .map(|t| match t {
                            Json::Object(t) => ordered(t, TRIGGER_FIRST, &[], &|_, v| plain(v)),
                            other => plain(other),
                        })
                        .collect(),
                ),
                other => plain(other),
            },
            _ => plain(v),
        }),
        _ => plain(v),
    })
}

/// A workflow document as YAML, in the conventional order.
pub fn to_yaml(document: &Json) -> anyhow::Result<String> {
    serde_saphyr::to_string(&order_document(document)).context("failed to write YAML")
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

    /// The organization's workflow folders.
    async fn folders(&self) -> anyhow::Result<Vec<Json>> {
        let all = self.get("/api/v1/workflow-folders").await?;
        Ok(all
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["orgId"] == self.org_id.as_str())
            .cloned()
            .collect())
    }

    /// The id of the folder at `path` (none: unfiled), made if it isn't
    /// there. Names match ignoring case, or as written by `pull`.
    async fn folder_at(
        &self,
        folders: &mut Vec<Json>,
        path: &[String],
    ) -> anyhow::Result<Option<String>> {
        let mut parent: Option<String> = None;
        for part in path {
            let existing = folders.iter().find(|f| {
                f["parentId"].as_str() == parent.as_deref()
                    && f["name"].as_str().is_some_and(|n| {
                        n.to_lowercase() == part.to_lowercase() || dir_name(n) == *part
                    })
            });
            let id = if let Some(f) = existing {
                f["id"].as_str().unwrap_or_default().to_owned()
            } else {
                let made = self
                    .post(
                        "/api/v1/workflow-folders",
                        &json!({ "orgId": self.org_id, "folder": { "name": part, "parentId": parent } }),
                    )
                    .await?;
                println!("made folder {}", made["name"].as_str().unwrap_or(part));
                let id = made["id"].as_str().unwrap_or_default().to_owned();
                folders.push(made);
                id
            };
            parent = Some(id);
        }
        Ok(parent)
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

/// The names from the top down to folder `id` (empty: unfiled), as
/// directory names.
fn folder_path(folders: &[Json], id: Option<&str>) -> Vec<String> {
    let mut path = Vec::new();
    let mut at = id;
    while let Some(id) = at {
        let Some(f) = folders.iter().find(|f| f["id"] == id) else {
            break;
        };
        path.push(dir_name(f["name"].as_str().unwrap_or_default()));
        at = f["parentId"].as_str();
        if path.len() > 64 {
            break;
        }
    }
    path.reverse();
    path
}

/// Each key's files among `found`, to catch a workflow in two files.
fn files_by_key(found: &[Found]) -> BTreeMap<String, Vec<PathBuf>> {
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for f in found {
        if let Ok(document) = load(&f.path)
            && let Ok(key) = key_of(&document, &f.path)
        {
            out.entry(key).or_default().push(f.path.clone());
        }
    }
    out
}

async fn pull(client: &Client, dir: &Path, keys: &[String]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let workflows = client.workflows().await?;
    let folders = client.folders().await?;
    // Files already here: a workflow that moved folder leaves its old file.
    let before = files_by_key(&found(&[dir.to_path_buf()])?);
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
        let mut path = dir.to_path_buf();
        for part in folder_path(&folders, summary["folderId"].as_str()) {
            path.push(part);
        }
        std::fs::create_dir_all(&path)?;
        path.push(format!("{key}.yaml"));
        let text = format!("{SCHEMA_COMMENT}\n{}", to_yaml(&detail["draft"])?);
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("pulled {}", path.display());
        for old in before.get(key).into_iter().flatten() {
            if *old != path {
                std::fs::remove_file(old)
                    .with_context(|| format!("failed to remove {}", old.display()))?;
                println!("removed {} (it moved)", old.display());
            }
        }
        written += 1;
    }
    println!("{written} workflow(s) pulled.");
    Ok(())
}

async fn diff(client: &Client, paths: &[PathBuf]) -> anyhow::Result<()> {
    let workflows = client.workflows().await?;
    let folders = client.folders().await?;
    let mut differ = 0;
    for Found { path, folder } in found(paths)? {
        let document = load(&path)?;
        let key = key_of(&document, &path)?;
        let local = to_yaml(&document)?;
        let Some(summary) = workflows.get(&key) else {
            println!("new   {} ({key} isn't on the server yet)", path.display());
            differ += 1;
            continue;
        };
        if let Some(folder) = &folder {
            let on_server = folder_path(&folders, summary["folderId"].as_str());
            if *folder != on_server {
                println!(
                    "folder {}: the server has it in {}, the files in {}",
                    path.display(),
                    show_folder(&on_server),
                    show_folder(folder)
                );
                differ += 1;
            }
        }
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
    let mut folders = client.folders().await?;
    let found = found(paths)?;
    if let Some((key, twice)) = files_by_key(&found).into_iter().find(|(_, p)| p.len() > 1) {
        let list: Vec<String> = twice.iter().map(|p| p.display().to_string()).collect();
        bail!("{key} is in more than one file: {}", list.join(", "));
    }
    for Found { path, folder } in found {
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
        if let Some(folder) = &folder {
            file_in(client, &mut folders, &saved, &key, folder).await?;
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

/// Moves a pushed workflow into the folder its file is in, if it isn't
/// there. A move the server refuses (built-in workflows stay in the System
/// folder) is reported, not fatal.
async fn file_in(
    client: &Client,
    folders: &mut Vec<Json>,
    saved: &Json,
    key: &str,
    folder: &[String],
) -> anyhow::Result<()> {
    let current = folder_path(folders, saved["folderId"].as_str());
    if current == folder {
        return Ok(());
    }
    let target = match client.folder_at(folders, folder).await {
        Ok(target) => target,
        Err(e) => {
            println!("  couldn't make folder {}: {e:#}", show_folder(folder));
            return Ok(());
        }
    };
    let body = json!({ "workflowIds": [saved["id"]], "folderId": target });
    match client.post("/api/v1/workflow-folders/move", &body).await {
        Ok(_) => println!("moved {key} to {}", show_folder(folder)),
        Err(e) => println!("  left {key} in {}: {e:#}", show_folder(&current)),
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
    let run = client.wait(&run_id, None).await?;
    match run["status"].as_str().unwrap_or_default() {
        "succeeded" => {
            println!("{}", serde_json::to_string_pretty(&run["outputs"])?);
            Ok(())
        }
        status => {
            eprintln!("{}", serde_json::to_string_pretty(&run["error"])?);
            bail!("run {run_id} {status}");
        }
    }
}

impl Client {
    /// Waits for run `run_id` to finish (at most `limit`), and returns it.
    async fn wait(&self, run_id: &str, limit: Option<Duration>) -> anyhow::Result<Json> {
        let started = std::time::Instant::now();
        let mut delay = Duration::from_millis(250);
        loop {
            let run = self.get(&format!("/api/v1/runs/{run_id}")).await?;
            if matches!(
                run["status"].as_str(),
                Some("succeeded" | "failed" | "cancelled")
            ) {
                return Ok(run);
            }
            if limit.is_some_and(|l| started.elapsed() > l) {
                bail!(
                    "run {run_id} didn't finish in time (it's {})",
                    run["status"]
                );
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(5));
        }
    }
}

// ---- fixture tests ----

/// A `*.test.yaml` file.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestFile {
    /// The workflow file, relative to the test file.
    workflow: PathBuf,
    cases: Vec<TestCase>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestCase {
    name: String,
    #[serde(default = "empty_object")]
    inputs: Json,
    /// Step outputs to use instead of running those steps.
    #[serde(default)]
    mocks: BTreeMap<String, Json>,
    #[serde(default)]
    expect: Expect,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    /// `succeeded` (the default), `failed` or `cancelled`.
    #[serde(default)]
    status: Option<String>,
    /// Matched as a subset: every key given must be there, equal.
    #[serde(default)]
    outputs: Option<Json>,
    #[serde(default)]
    error: Option<Json>,
}

fn empty_object() -> Json {
    json!({})
}

/// Whether `actual` has everything in `expected`: objects match key by
/// key (extra keys are fine), everything else exactly.
fn matches(expected: &Json, actual: &Json) -> bool {
    match (expected, actual) {
        (Json::Object(e), Json::Object(a)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|av| matches(v, av))),
        _ => expected == actual,
    }
}

/// What's wrong with `run` for `expect`, if anything.
fn check(expect: &Expect, run: &Json) -> Vec<String> {
    let mut problems = Vec::new();
    let status = expect.status.as_deref().unwrap_or("succeeded");
    if run["status"] != status {
        problems.push(format!(
            "status is {}, expected {status}; error: {}",
            run["status"], run["error"]
        ));
    }
    if let Some(outputs) = &expect.outputs
        && !matches(outputs, &run["outputs"])
    {
        problems.push(format!(
            "outputs are {}, expected {outputs}",
            run["outputs"]
        ));
    }
    if let Some(error) = &expect.error
        && !matches(error, &run["error"])
    {
        problems.push(format!("error is {}, expected {error}", run["error"]));
    }
    problems
}

fn test_files(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && is_test_file(p))
                .collect();
            found.sort();
            out.extend(found);
        } else {
            out.push(path.clone());
        }
    }
    if out.is_empty() {
        bail!("no *.test.yaml files found");
    }
    Ok(out)
}

async fn test(client: &Client, paths: &[PathBuf], timeout: Duration) -> anyhow::Result<()> {
    let (mut passed, mut failed) = (0, 0);
    for path in test_files(paths)? {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let file: TestFile = serde_saphyr::from_str(&text)
            .with_context(|| format!("{} isn't a valid test file", path.display()))?;
        let workflow_path = path.parent().unwrap_or(Path::new(".")).join(&file.workflow);
        let document = load(&workflow_path)?;
        println!("{}", path.display());
        for case in &file.cases {
            let body = json!({ "orgId": client.org_id, "document": document,
                               "inputs": case.inputs, "mocks": case.mocks });
            let problems = match client.post("/api/v1/workflows/test-runs", &body).await {
                Ok(started) => {
                    let run_id = started["id"].as_str().unwrap_or_default().to_owned();
                    match client.wait(&run_id, Some(timeout)).await {
                        Ok(run) => check(&case.expect, &run),
                        Err(e) => vec![format!("{e:#}")],
                    }
                }
                Err(e) => vec![format!("{e:#}")],
            };
            if problems.is_empty() {
                println!("  ok    {}", case.name);
                passed += 1;
            } else {
                println!("  FAIL  {}", case.name);
                for p in &problems {
                    println!("        {p}");
                }
                failed += 1;
            }
        }
    }
    println!("{passed} passed, {failed} failed");
    if failed > 0 {
        bail!("{failed} test case(s) failed");
    }
    Ok(())
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
            "apiVersion": "usehuxley.io/v1"
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
    fn expectations_match_subsets() {
        assert!(matches(&json!({ "a": 1 }), &json!({ "a": 1, "b": 2 })));
        assert!(matches(
            &json!({ "a": { "x": 1 } }),
            &json!({ "a": { "x": 1, "y": 2 } })
        ));
        assert!(!matches(&json!({ "a": 1 }), &json!({ "a": 2 })));
        assert!(!matches(&json!({ "a": 1 }), &json!(null)));
        assert!(!matches(&json!([1]), &json!([1, 2])));
        let run = json!({ "status": "failed", "error": { "kind": "network", "message": "x" }, "outputs": null });
        let expect = Expect {
            status: Some("failed".into()),
            outputs: None,
            error: Some(json!({ "kind": "network" })),
        };
        assert_eq!(check(&expect, &run), Vec::<String>::new());
        assert_eq!(check(&Expect::default(), &run).len(), 1);
        assert!(is_test_file(Path::new("a/greet.test.yaml")));
        assert!(!is_workflow_file(Path::new("a/greet.test.yaml")));
        assert!(is_workflow_file(Path::new("a/greet.yaml")));
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

/// `pull`, `diff` and `push` against a real API on localhost: folders
/// become directories and back.
#[cfg(test)]
mod folder_tests {
    use super::*;
    use huxley_core::{RequestContext, Via};
    use huxley_db::testing::{ensure_root, isolated};
    use huxley_db::{OrgScope, begin_scoped};

    /// A server, an owner's token, and the root organization's id.
    async fn server() -> Option<(huxley_db::testing::IsolatedDb, Connection)> {
        let db = isolated("cli_folders").await?;
        let pools = db.pools.clone();
        let root = ensure_root(&pools).await;
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
        let user = huxley_auth::create_user(
            &mut tx,
            &ctx,
            "cli@example.com",
            "CLI",
            "cli folders passphrase",
        )
        .await
        .unwrap();
        huxley_authz::grant_root_owner(&mut tx, &ctx, user.id)
            .await
            .unwrap();
        let token = huxley_auth::api_tokens::create(
            &mut tx,
            &ctx,
            &user,
            "cli",
            &["*".to_owned()],
            Some(time::OffsetDateTime::now_utc() + time::Duration::days(1)),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let crypto =
            || huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
        let auth = huxley_auth::AuthService::new(
            pools.app.clone(),
            huxley_auth::AuthConfig::default(),
            crypto(),
        );
        let state = huxley_api::AppState::new(
            pools.app.clone(),
            auth,
            crypto(),
            huxley_api::ApiConfig::default(),
        );
        let app =
            huxley_api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Some((
            db,
            Connection {
                server: format!("http://127.0.0.1:{port}"),
                token: token.secret,
                org: root.to_string(),
            },
        ))
    }

    fn workflow(key: &str) -> Json {
        json!({
            "apiVersion": "usehuxley.io/v1", "kind": "Workflow",
            "metadata": { "key": key, "name": key },
            "spec": { "steps": [{ "id": "a", "type": "transform", "output": "x" }] },
        })
    }

    async fn folder_of(client: &Client, key: &str) -> String {
        let workflows = client.workflows().await.unwrap();
        let folders = client.folders().await.unwrap();
        show_folder(&folder_path(&folders, workflows[key]["folderId"].as_str()))
    }

    #[tokio::test]
    async fn folders_are_directories() {
        let Some((_db, connection)) = server().await else {
            return;
        };
        let client = Client::new(&connection).await.unwrap();
        for key in ["alpha", "beta"] {
            client
                .post(
                    "/api/v1/workflows",
                    &json!({ "orgId": client.org_id, "document": workflow(key) }),
                )
                .await
                .unwrap();
        }
        let mut folders = client.folders().await.unwrap();
        let ops = client
            .folder_at(&mut folders, &["Ops".to_owned()])
            .await
            .unwrap();
        let alpha = client.workflows().await.unwrap()["alpha"]["id"].clone();
        client
            .post(
                "/api/v1/workflow-folders/move",
                &json!({ "workflowIds": [alpha], "folderId": ops }),
            )
            .await
            .unwrap();

        // Pulled into directories.
        let dir = std::env::temp_dir().join(format!("huxley-cli-{}", uuid::Uuid::now_v7()));
        pull(&client, &dir, &[]).await.unwrap();
        assert!(dir.join("Ops/alpha.yaml").is_file());
        assert!(dir.join("beta.yaml").is_file());
        diff(&client, std::slice::from_ref(&dir)).await.unwrap();

        // Moving files moves the workflows (making folders as needed).
        std::fs::create_dir_all(dir.join("Ops/Daily")).unwrap();
        std::fs::rename(dir.join("beta.yaml"), dir.join("Ops/Daily/beta.yaml")).unwrap();
        std::fs::rename(dir.join("Ops/alpha.yaml"), dir.join("alpha.yaml")).unwrap();
        assert!(diff(&client, std::slice::from_ref(&dir)).await.is_err());
        push(&client, std::slice::from_ref(&dir), false, None)
            .await
            .unwrap();
        assert_eq!(folder_of(&client, "beta").await, "Ops/Daily");
        assert_eq!(folder_of(&client, "alpha").await, "unfiled");
        diff(&client, std::slice::from_ref(&dir)).await.unwrap();
        // A file named on its own keeps its workflow where it is.
        push(&client, &[dir.join("alpha.yaml")], false, None)
            .await
            .unwrap();
        assert_eq!(folder_of(&client, "beta").await, "Ops/Daily");

        // Moved on the server: pulling moves the file.
        let beta = client.workflows().await.unwrap()["beta"]["id"].clone();
        client
            .post(
                "/api/v1/workflow-folders/move",
                &json!({ "workflowIds": [beta], "folderId": null }),
            )
            .await
            .unwrap();
        pull(&client, &dir, &[]).await.unwrap();
        assert!(dir.join("beta.yaml").is_file());
        assert!(!dir.join("Ops/Daily/beta.yaml").exists());

        // The same workflow in two files is refused.
        std::fs::copy(dir.join("alpha.yaml"), dir.join("Ops/alpha.yaml")).unwrap();
        let err = push(&client, std::slice::from_ref(&dir), false, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("more than one file"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folder_names_make_safe_directories() {
        assert_eq!(dir_name("Ops / Daily"), "Ops - Daily");
        assert_eq!(dir_name(".hidden"), "_hidden");
        assert_eq!(dir_name(".."), "_");
        assert_eq!(dir_name("System"), "System");
    }
}
