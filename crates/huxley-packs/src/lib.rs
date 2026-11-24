//! Packs (design doc §6.6, ADR-028): a `.hxpack` is a zip whose
//! `manifest.json` lists every other file with its SHA-256 and names what
//! the pack adds; `signature.json` (optional) is an Ed25519 signature over
//! the manifest's exact bytes. Reading is strict: sizes are bounded, paths
//! stay in known folders, and every file must match its hash.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::digest::{SHA256, digest};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub mod builtin;

/// The largest pack, unpacked.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
/// The most files in a pack.
pub const MAX_FILES: usize = 500;
const MANIFEST: &str = "manifest.json";
const SIGNATURE: &str = "signature.json";
/// Where content files may be.
const FOLDERS: &[&str] = &[
    "integrations/",
    "workflows/",
    "forms/",
    "scripts/",
    "templates/",
    "docs/",
];
/// What a public key looks like as text.
pub const KEY_PREFIX: &str = "hxpk_";

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("this isn't a pack: {0}")]
    Format(String),
    #[error("the pack is larger than 20 MB or has more than 500 files")]
    TooLarge,
    #[error("{0:?} isn't allowed in a pack")]
    BadPath(String),
    #[error("{0} doesn't match the manifest (changed after it was built?)")]
    Tampered(String),
    #[error("the manifest lists {0}, which the pack doesn't have")]
    Missing(String),
    #[error("{0} is in the pack but not in its manifest")]
    Unlisted(String),
    #[error("the pack's signature is invalid")]
    BadSignature,
    #[error("bad key: {0}")]
    BadKey(String),
}

pub type PackResult<T> = Result<T, PackError>;

/// What a pack adds, by key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integrations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<String>,
}

/// A pack's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub api_version: String,
    pub kind: String,
    /// e.g. `huxley/msgraph` or `contoso/onboarding`.
    pub key: String,
    pub name: String,
    /// Semver, e.g. `1.2.0`.
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub publisher: String,
    pub contents: Contents,
    /// Every other file, with its SHA-256 (hex).
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// `signature.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signature {
    pub algorithm: String,
    /// The signing key, as `hxpk_…`.
    pub key: String,
    pub publisher: String,
    /// Base64 Ed25519 signature over `manifest.json`'s bytes.
    pub signature: String,
}

/// Who vouches for a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Trust {
    /// Ships with Huxley.
    BuiltIn {
        publisher: String,
    },
    /// Signed by a key this instance trusts.
    Trusted {
        publisher: String,
        key: String,
    },
    /// A valid signature, by a key this instance doesn't trust (yet).
    UnknownSigner {
        publisher: String,
        key: String,
    },
    Unsigned,
}

/// A pack, read and checked.
#[derive(Debug, Clone)]
pub struct Pack {
    pub manifest: Manifest,
    pub signature: Option<Signature>,
    manifest_bytes: Vec<u8>,
    files: BTreeMap<String, Vec<u8>>,
    builtin: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(64);
    for b in digest(&SHA256, bytes).as_ref() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn check_path(path: &str) -> PackResult<()> {
    let ok = FOLDERS.iter().any(|f| path.starts_with(f))
        && !path.contains("..")
        && !path.contains('\\')
        && !path.contains("//")
        && !path.ends_with('/')
        && path.len() <= 200
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(PackError::BadPath(path.to_owned()))
    }
}

fn valid_key(key: &str) -> bool {
    let mut parts = key.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|p| {
        (1..=64).contains(&p.len())
            && p.starts_with(|c: char| c.is_ascii_lowercase())
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

fn valid_version(v: &str) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

impl Manifest {
    fn check(&self) -> PackResult<()> {
        if self.api_version != "usehuxley.io/v1" || self.kind != "Pack" {
            return Err(PackError::Format(
                "apiVersion usehuxley.io/v1, kind Pack".into(),
            ));
        }
        if !valid_key(&self.key) {
            return Err(PackError::Format("key is <owner>/<name>, lowercase".into()));
        }
        if !valid_version(&self.version) {
            return Err(PackError::Format("version is semver, e.g. 1.0.0".into()));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 200 {
            return Err(PackError::Format("name is 1 to 200 characters".into()));
        }
        Ok(())
    }

    /// The files each content item needs.
    fn required_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(
            self.contents
                .integrations
                .iter()
                .map(|k| format!("integrations/{k}.yaml")),
        );
        out.extend(
            self.contents
                .workflows
                .iter()
                .map(|k| format!("workflows/{k}.yaml")),
        );
        out.extend(
            self.contents
                .forms
                .iter()
                .map(|k| format!("forms/{k}.json")),
        );
        out.extend(
            self.contents
                .scripts
                .iter()
                .map(|k| format!("scripts/{k}.json")),
        );
        out.extend(
            self.contents
                .templates
                .iter()
                .map(|k| format!("templates/{k}.yaml")),
        );
        out
    }
}

impl Pack {
    /// Reads and checks a `.hxpack`. The signature is checked if present
    /// (see [`Pack::trust`] for whether its key is trusted).
    pub fn read(bytes: &[u8]) -> PackResult<Self> {
        let mut pack = Self::read_unrecognised(bytes)?;
        pack.builtin = builtin::is_builtin(&pack);
        Ok(pack)
    }

    fn read_unrecognised(bytes: &[u8]) -> PackResult<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(PackError::TooLarge);
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| PackError::Format(e.to_string()))?;
        if archive.len() > MAX_FILES + 2 {
            return Err(PackError::TooLarge);
        }
        let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut total = 0usize;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| PackError::Format(e.to_string()))?;
            if file.is_dir() {
                continue;
            }
            let name = file
                .name()
                .map_err(|e| PackError::Format(e.to_string()))?
                .into_owned();
            if name != MANIFEST && name != SIGNATURE {
                check_path(&name)?;
            }
            let mut content = Vec::new();
            // Bounded even if the zip lies about sizes.
            let limit = u64::try_from(MAX_BYTES - total).unwrap_or(0) + 1;
            (&mut file)
                .take(limit)
                .read_to_end(&mut content)
                .map_err(|e| PackError::Format(e.to_string()))?;
            total += content.len();
            if total > MAX_BYTES {
                return Err(PackError::TooLarge);
            }
            if entries.insert(name.clone(), content).is_some() {
                return Err(PackError::Format(format!("{name} appears twice")));
            }
        }
        let manifest_bytes = entries
            .remove(MANIFEST)
            .ok_or_else(|| PackError::Format("no manifest.json".into()))?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| PackError::Format(format!("manifest.json: {e}")))?;
        manifest.check()?;
        let signature = match entries.remove(SIGNATURE) {
            Some(raw) => Some(
                serde_json::from_slice::<Signature>(&raw)
                    .map_err(|e| PackError::Format(format!("signature.json: {e}")))?,
            ),
            None => None,
        };
        for (path, hash) in &manifest.files {
            check_path(path)?;
            let content = entries
                .get(path)
                .ok_or_else(|| PackError::Missing(path.clone()))?;
            if sha256_hex(content) != hash.to_ascii_lowercase() {
                return Err(PackError::Tampered(path.clone()));
            }
        }
        if let Some(extra) = entries.keys().find(|p| !manifest.files.contains_key(*p)) {
            return Err(PackError::Unlisted(extra.clone()));
        }
        for path in manifest.required_files() {
            if !entries.contains_key(&path) {
                return Err(PackError::Missing(path));
            }
        }
        let pack = Self {
            manifest,
            signature,
            manifest_bytes,
            files: entries,
            builtin: false,
        };
        if let Some(signature) = &pack.signature {
            verify(&pack.manifest_bytes, signature)?;
        }
        Ok(pack)
    }

    /// A file's contents.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// A file as text.
    pub fn text(&self, path: &str) -> Option<&str> {
        self.file(path).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Every content file, by path.
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    /// The same pack, signed with `key` for `publisher` (replacing any
    /// signature it had).
    pub fn sign(&self, key: &SigningKey, publisher: &str) -> PackResult<Vec<u8>> {
        build(self.manifest.clone(), &self.files, Some((key, publisher)))
    }

    /// Whether `trusted` keys (`hxpk_…`) vouch for it.
    pub fn trust(&self, trusted: &[String]) -> Trust {
        if self.builtin {
            return Trust::BuiltIn {
                publisher: self.manifest.publisher.clone(),
            };
        }
        match &self.signature {
            None => Trust::Unsigned,
            Some(s) if trusted.iter().any(|k| k.trim() == s.key) => Trust::Trusted {
                publisher: s.publisher.clone(),
                key: s.key.clone(),
            },
            Some(s) => Trust::UnknownSigner {
                publisher: s.publisher.clone(),
                key: s.key.clone(),
            },
        }
    }
}

fn verify(manifest: &[u8], signature: &Signature) -> PackResult<()> {
    if signature.algorithm != "ed25519" {
        return Err(PackError::BadSignature);
    }
    let public = public_key_bytes(&signature.key)?;
    let sig = STANDARD
        .decode(&signature.signature)
        .map_err(|_| PackError::BadSignature)?;
    UnparsedPublicKey::new(&ED25519, public)
        .verify(manifest, &sig)
        .map_err(|_| PackError::BadSignature)
}

fn public_key_bytes(key: &str) -> PackResult<Vec<u8>> {
    let raw = key
        .trim()
        .strip_prefix(KEY_PREFIX)
        .ok_or_else(|| PackError::BadKey("public keys start with hxpk_".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| PackError::BadKey("not base64url".into()))?;
    if bytes.len() == 32 {
        Ok(bytes)
    } else {
        Err(PackError::BadKey(
            "an Ed25519 public key is 32 bytes".into(),
        ))
    }
}

/// A signing key: its private half (PKCS#8, base64) and public key.
#[derive(Clone)]
pub struct SigningKey {
    pkcs8: Vec<u8>,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("public", &self.public())
            .finish_non_exhaustive()
    }
}

const PRIVATE_HEADER: &str = "huxley-pack-signing-key v1";

impl SigningKey {
    pub fn generate() -> PackResult<Self> {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 =
            Ed25519KeyPair::generate_pkcs8(&rng).map_err(|e| PackError::BadKey(e.to_string()))?;
        Ok(Self {
            pkcs8: pkcs8.as_ref().to_vec(),
        })
    }

    /// The key file's text (keep it secret).
    pub fn to_text(&self) -> String {
        format!("{PRIVATE_HEADER}\n{}\n", STANDARD.encode(&self.pkcs8))
    }

    pub fn from_text(text: &str) -> PackResult<Self> {
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some(PRIVATE_HEADER) {
            return Err(PackError::BadKey("not a Huxley pack signing key".into()));
        }
        let pkcs8 = STANDARD
            .decode(lines.next().unwrap_or_default().trim())
            .map_err(|_| PackError::BadKey("damaged key file".into()))?;
        Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|e| PackError::BadKey(e.to_string()))?;
        Ok(Self { pkcs8 })
    }

    fn pair(&self) -> Ed25519KeyPair {
        Ed25519KeyPair::from_pkcs8(&self.pkcs8).expect("checked when loaded")
    }

    /// `hxpk_…`, to share with instances that should trust it.
    pub fn public(&self) -> String {
        format!(
            "{KEY_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(self.pair().public_key().as_ref())
        )
    }
}

/// Builds a `.hxpack` from `manifest` (its `files` are filled in) and the
/// content `files` by path, signed with `key` if given.
pub fn build(
    mut manifest: Manifest,
    files: &BTreeMap<String, Vec<u8>>,
    key: Option<(&SigningKey, &str)>,
) -> PackResult<Vec<u8>> {
    manifest.check()?;
    if files.len() > MAX_FILES {
        return Err(PackError::TooLarge);
    }
    manifest.files.clear();
    for (path, content) in files {
        check_path(path)?;
        manifest.files.insert(path.clone(), sha256_hex(content));
    }
    for path in manifest.required_files() {
        if !files.contains_key(&path) {
            return Err(PackError::Missing(path));
        }
    }
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifests serialize");
    let mut out = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, bytes: &[u8]| -> PackResult<()> {
            zip.start_file(name, options)
                .map_err(|e| PackError::Format(e.to_string()))?;
            zip.write_all(bytes)
                .map_err(|e| PackError::Format(e.to_string()))
        };
        add(MANIFEST, &manifest_bytes)?;
        if let Some((key, publisher)) = key {
            let signature = Signature {
                algorithm: "ed25519".into(),
                key: key.public(),
                publisher: publisher.to_owned(),
                signature: STANDARD.encode(key.pair().sign(&manifest_bytes).as_ref()),
            };
            add(
                SIGNATURE,
                &serde_json::to_vec_pretty(&signature).expect("signatures serialize"),
            )?;
        }
        for (path, content) in files {
            add(path, content)?;
        }
        zip.finish().map_err(|e| PackError::Format(e.to_string()))?;
    }
    let bytes = out.into_inner();
    if bytes.len() > MAX_BYTES {
        return Err(PackError::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            api_version: "usehuxley.io/v1".into(),
            kind: "Pack".into(),
            key: "contoso/onboarding".into(),
            name: "Onboarding".into(),
            version: "1.0.0".into(),
            description: String::new(),
            publisher: "Contoso".into(),
            contents: Contents {
                workflows: vec!["onboard".into()],
                ..Contents::default()
            },
            files: BTreeMap::new(),
        }
    }

    fn files() -> BTreeMap<String, Vec<u8>> {
        [(
            "workflows/onboard.yaml".to_owned(),
            b"apiVersion: usehuxley.io/v1\n".to_vec(),
        )]
        .into()
    }

    /// Rewrites one entry of a built pack (as an attacker would).
    fn rewrite(pack: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(Cursor::new(pack)).unwrap();
        let mut out = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            for i in 0..archive.len() {
                let mut file = archive.by_index(i).unwrap();
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                let entry = file.name().unwrap().into_owned();
                zip.start_file(&entry, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(if entry == name { content } else { &bytes })
                    .unwrap();
            }
            if archive.by_name(name).is_err() {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(content).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn only_the_current_api_version_is_accepted() {
        assert!(manifest().check().is_ok());
        for old in ["usehuxley.com/v1", "usehuxley.io/v2", ""] {
            let mut m = manifest();
            m.api_version = old.into();
            assert!(
                matches!(m.check(), Err(PackError::Format(_))),
                "{old:?} was accepted"
            );
        }
    }

    #[test]
    fn signed_packs_round_trip_and_trust_is_reported() {
        let key = SigningKey::generate().unwrap();
        let bytes = build(manifest(), &files(), Some((&key, "Contoso"))).unwrap();
        let pack = Pack::read(&bytes).unwrap();
        assert_eq!(pack.manifest.key, "contoso/onboarding");
        assert_eq!(
            pack.text("workflows/onboard.yaml"),
            Some("apiVersion: usehuxley.io/v1\n")
        );
        assert_eq!(
            pack.trust(&[key.public()]),
            Trust::Trusted {
                publisher: "Contoso".into(),
                key: key.public()
            }
        );
        assert!(matches!(pack.trust(&[]), Trust::UnknownSigner { .. }));
        let unsigned = Pack::read(&build(manifest(), &files(), None).unwrap()).unwrap();
        assert_eq!(unsigned.trust(&[key.public()]), Trust::Unsigned);
        // The key file round-trips.
        let again = SigningKey::from_text(&key.to_text()).unwrap();
        assert_eq!(again.public(), key.public());
    }

    #[test]
    fn packs_can_be_signed_later() {
        let unsigned = Pack::read(&build(manifest(), &files(), None).unwrap()).unwrap();
        let key = SigningKey::generate().unwrap();
        let signed = Pack::read(&unsigned.sign(&key, "Contoso").unwrap()).unwrap();
        assert_eq!(
            signed.trust(&[key.public()]),
            Trust::Trusted {
                publisher: "Contoso".into(),
                key: key.public()
            }
        );
        assert_eq!(signed.files(), unsigned.files());
        assert_eq!(signed.manifest, unsigned.manifest);
    }

    #[test]
    fn tampering_is_caught() {
        let key = SigningKey::generate().unwrap();
        let bytes = build(manifest(), &files(), Some((&key, "Contoso"))).unwrap();
        // A changed file no longer matches its hash.
        let changed = rewrite(&bytes, "workflows/onboard.yaml", b"evil: true\n");
        assert!(matches!(Pack::read(&changed), Err(PackError::Tampered(_))));
        // A file slipped in that the manifest doesn't list.
        let added = rewrite(&bytes, "scripts/extra.py", b"print(1)");
        assert!(matches!(Pack::read(&added), Err(PackError::Unlisted(_))));
        // A manifest edited to match a changed file breaks the signature.
        let pack = Pack::read(&bytes).unwrap();
        let mut forged = pack.manifest.clone();
        forged
            .files
            .insert("workflows/onboard.yaml".into(), sha256_hex(b"evil: true\n"));
        let forged_bytes = serde_json::to_vec_pretty(&forged).unwrap();
        let with_manifest = rewrite(&bytes, MANIFEST, &forged_bytes);
        let both = rewrite(&with_manifest, "workflows/onboard.yaml", b"evil: true\n");
        assert!(matches!(Pack::read(&both), Err(PackError::BadSignature)));
    }

    #[test]
    fn paths_and_manifests_are_checked() {
        for bad in [
            "../etc/passwd",
            "workflows/../x",
            "/abs",
            "other/x.yaml",
            "workflows/a b",
            "workflows\\x",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
        assert!(check_path("workflows/onboard.yaml").is_ok());
        let traversal = rewrite(&build(manifest(), &files(), None).unwrap(), "../evil", b"x");
        assert!(matches!(Pack::read(&traversal), Err(PackError::BadPath(_))));
        let mut no_file = manifest();
        no_file.contents.forms = vec!["missing".into()];
        assert!(matches!(
            build(no_file, &files(), None),
            Err(PackError::Missing(_))
        ));
        for (key, version) in [
            ("noslash", "1.0.0"),
            ("a/B", "1.0.0"),
            ("a/b", "1.0"),
            ("a/b", "x.y.z"),
        ] {
            let mut m = manifest();
            m.key = key.into();
            m.version = version.into();
            assert!(build(m, &files(), None).is_err(), "{key} {version}");
        }
        assert!(Pack::read(b"not a zip").is_err());
    }
}
