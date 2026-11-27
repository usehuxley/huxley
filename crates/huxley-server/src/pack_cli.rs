//! `huxley pack`: make signing keys, build packs from a folder, and look
//! inside a pack (ADR-028).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Subcommand;
use huxley_packs::{Manifest, Pack, SigningKey, Trust};

#[derive(Debug, Subcommand)]
pub enum PackCommand {
    /// Make a signing key. Keep the file secret; share the public key it prints.
    Keygen {
        /// Where to write the private key.
        #[arg(long)]
        out: PathBuf,
    },
    /// Build a pack from a folder: `pack.yaml` (the manifest, without
    /// `files`) and the integrations/, workflows/, forms/, scripts/,
    /// templates/ and docs/ folders.
    Build {
        dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Sign it with this key file.
        #[arg(long)]
        key: Option<PathBuf>,
    },
    /// Sign a pack (e.g. one exported from Huxley), replacing any signature.
    Sign {
        file: PathBuf,
        #[arg(long)]
        key: PathBuf,
        /// Who signs it (default: the manifest's publisher).
        #[arg(long)]
        publisher: Option<String>,
        /// Where to write the signed pack (default: in place).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Show what a pack contains and who signed it.
    Inspect {
        file: PathBuf,
        /// Public keys (hxpk_…) to treat as trusted.
        #[arg(long)]
        trust: Vec<String>,
    },
    /// Write a registry index (index.json) for the .hxpack files in a
    /// folder, to host the folder as a pack registry (ADR-093). Each pack
    /// is checked as an install would check it.
    Index {
        dir: PathBuf,
        /// Where the folder is served, e.g. `https://packs.example.com`.
        #[arg(long)]
        base_url: String,
        /// The registry's name, shown to admins.
        #[arg(long, default_value = "Packs")]
        name: String,
        /// Sign the index with this key file (writes index.json.sig);
        /// instances configured with the key refuse anything else.
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

pub fn run(command: PackCommand) -> anyhow::Result<()> {
    match command {
        PackCommand::Keygen { out } => keygen(&out),
        PackCommand::Build { dir, out, key } => build(&dir, &out, key.as_deref()),
        PackCommand::Sign {
            file,
            key,
            publisher,
            out,
        } => sign(file, &key, publisher, out),
        PackCommand::Inspect { file, trust } => inspect(&file, &trust),
        PackCommand::Index {
            dir,
            base_url,
            name,
            key,
        } => index(&dir, &base_url, &name, key.as_deref()),
    }
}

fn index(dir: &Path, base_url: &str, name: &str, key: Option<&Path>) -> anyhow::Result<()> {
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("hxpack") {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|f| f.to_str())
            .context("pack file names must be UTF-8")?
            .to_owned();
        files.push((file, std::fs::read(&path)?));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let index = huxley_packs::registry::build_index(name, base_url, &files)?;
    let bytes = serde_json::to_vec_pretty(&index)?;
    std::fs::write(dir.join("index.json"), &bytes)?;
    let versions: usize = index.packs.iter().map(|p| p.versions.len()).sum();
    println!(
        "Wrote {}: {} packs, {versions} versions.",
        dir.join("index.json").display(),
        index.packs.len()
    );
    if let Some(key) = key {
        let key = SigningKey::from_text(
            &std::fs::read_to_string(key)
                .with_context(|| format!("failed to read {}", key.display()))?,
        )?;
        std::fs::write(
            dir.join("index.json.sig"),
            huxley_packs::registry::sign_index(&bytes, &key, name)?,
        )?;
        println!(
            "Signed it: instances that set this registry's key to {} accept it.",
            key.public()
        );
    }
    Ok(())
}

fn keygen(out: &Path) -> anyhow::Result<()> {
    if out.exists() {
        bail!("{} already exists", out.display());
    }
    let key = SigningKey::generate()?;
    std::fs::write(out, key.to_text())
        .with_context(|| format!("failed to write {}", out.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o600))?;
    }
    println!(
        "Wrote {}. Public key (give it to instances that should trust you):",
        out.display()
    );
    println!("{}", key.public());
    Ok(())
}

fn build(dir: &Path, out: &Path, key: Option<&Path>) -> anyhow::Result<()> {
    let manifest_text = std::fs::read_to_string(dir.join("pack.yaml"))
        .with_context(|| format!("no pack.yaml in {}", dir.display()))?;
    let manifest: Manifest =
        serde_saphyr::from_str(&manifest_text).context("pack.yaml isn't a valid manifest")?;
    let mut files = BTreeMap::new();
    for folder in [
        "integrations",
        "workflows",
        "forms",
        "scripts",
        "templates",
        "docs",
    ] {
        collect(dir, &dir.join(folder), &mut files)?;
    }
    let key = match key {
        Some(path) => Some(SigningKey::from_text(
            &std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?,
        )?),
        None => None,
    };
    let publisher = manifest.publisher.clone();
    let bytes = huxley_packs::build(
        manifest,
        &files,
        key.as_ref().map(|k| (k, publisher.as_str())),
    )?;
    std::fs::write(out, &bytes).with_context(|| format!("failed to write {}", out.display()))?;
    println!(
        "Built {} ({} files, {} bytes){}.",
        out.display(),
        files.len(),
        bytes.len(),
        if key.is_some() {
            ", signed"
        } else {
            ", unsigned"
        }
    );
    Ok(())
}

fn sign(
    file: PathBuf,
    key: &Path,
    publisher: Option<String>,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let bytes =
        std::fs::read(&file).with_context(|| format!("failed to read {}", file.display()))?;
    let pack = Pack::read(&bytes)?;
    let key = SigningKey::from_text(
        &std::fs::read_to_string(key)
            .with_context(|| format!("failed to read {}", key.display()))?,
    )?;
    let publisher = publisher.unwrap_or_else(|| pack.manifest.publisher.clone());
    let signed = pack.sign(&key, &publisher)?;
    let out = out.unwrap_or(file);
    std::fs::write(&out, signed).with_context(|| format!("failed to write {}", out.display()))?;
    println!(
        "Signed {} as {publisher} with {}.",
        out.display(),
        key.public()
    );
    Ok(())
}

fn inspect(file: &Path, trust: &[String]) -> anyhow::Result<()> {
    let bytes =
        std::fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;
    let pack = Pack::read(&bytes)?;
    let m = &pack.manifest;
    println!("{} {} ({})", m.name, m.version, m.key);
    if !m.description.is_empty() {
        println!("{}", m.description);
    }
    match pack.trust(trust) {
        Trust::BuiltIn { publisher } => println!("Built in, from {publisher}."),
        Trust::Trusted { publisher, .. } => println!("Signed by {publisher} (trusted)."),
        Trust::UnknownSigner { publisher, key } => {
            println!("Signed by {publisher} with a key you don't trust yet: {key}");
        }
        Trust::Unsigned => println!("Unsigned."),
    }
    let c = &m.contents;
    for (kind, keys) in [
        ("Integrations", &c.integrations),
        ("Workflows", &c.workflows),
        ("Forms", &c.forms),
        ("Scripts", &c.scripts),
        ("Templates", &c.templates),
    ] {
        if !keys.is_empty() {
            println!("{kind}: {}", keys.join(", "));
        }
    }
    Ok(())
}

/// Adds the files under `folder` to `files`, by path relative to `root`.
fn collect(
    root: &Path,
    folder: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> anyhow::Result<()> {
    if !folder.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(folder)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            files.insert(relative, std::fs::read(&path)?);
        }
    }
    Ok(())
}
