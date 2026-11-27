//! Pack registries (ADR-093): a static `index.json` listing packs and their
//! versions, each with where to download it, its SHA-256 and who signed it,
//! optionally signed as a whole (`index.json.sig`) by the registry's key.
//! Any web server can host one: the community registry, or an MSP's own.
//! Huxley checks a downloaded pack's hash against the index, then reviews it
//! like an upload, so a registry can't skip the review or the signature
//! checks a pack gets.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Contents, Pack, PackError, PackResult, Signature, SigningKey, sha256_hex};

pub const INDEX_KIND: &str = "PackIndex";

/// A registry this instance browses (`[[packs.registries]]`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryConfig {
    /// The index, e.g. `https://packs.example.com/index.json`.
    pub url: String,
    /// The registry's key (`hxpk_…`). When set, the index must carry its
    /// signature (`index.json.sig`): a tampered or swapped index is refused.
    #[serde(default)]
    pub key: Option<String>,
}

/// `index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Index {
    pub api_version: String,
    pub kind: String,
    /// The registry's name, shown to admins.
    pub name: String,
    pub packs: Vec<IndexedPack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedPack {
    pub key: String,
    /// The newest version's name and description.
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub publisher: String,
    /// Newest first.
    pub versions: Vec<IndexedVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedVersion {
    pub version: String,
    /// The `.hxpack`: absolute, or relative to the index.
    pub url: String,
    /// SHA-256 of the file (hex): a download that doesn't match is refused.
    pub sha256: String,
    pub size: u64,
    /// The key (`hxpk_…`) that signed it, if any.
    #[serde(default)]
    pub signed_by: Option<String>,
    pub contents: Contents,
}

/// Builds an index from `.hxpack` files (file name, bytes), each read and
/// checked as an install would. Downloads are `<base_url>/<file name>`.
pub fn build_index(name: &str, base_url: &str, files: &[(String, Vec<u8>)]) -> PackResult<Index> {
    let mut by_key: BTreeMap<String, Vec<(Pack, IndexedVersion)>> = BTreeMap::new();
    for (file, bytes) in files {
        let pack = Pack::read(bytes).map_err(|e| PackError::Index(format!("{file}: {e}")))?;
        let version = IndexedVersion {
            version: pack.manifest.version.clone(),
            url: format!("{}/{file}", base_url.trim_end_matches('/')),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
            signed_by: pack.signature.as_ref().map(|s| s.key.clone()),
            contents: pack.manifest.contents.clone(),
        };
        by_key
            .entry(pack.manifest.key.clone())
            .or_default()
            .push((pack, version));
    }
    let mut packs = Vec::new();
    for (key, mut versions) in by_key {
        versions.sort_by(|(a, _), (b, _)| {
            semver(&b.manifest.version).cmp(&semver(&a.manifest.version))
        });
        if let Some(w) = versions
            .windows(2)
            .find(|w| w[0].1.version == w[1].1.version)
        {
            return Err(PackError::Index(format!(
                "{key} {} is in the folder twice",
                w[0].1.version
            )));
        }
        let newest = &versions[0].0.manifest;
        packs.push(IndexedPack {
            key,
            name: newest.name.clone(),
            description: newest.description.clone(),
            publisher: newest.publisher.clone(),
            versions: versions.into_iter().map(|(_, v)| v).collect(),
        });
    }
    Ok(Index {
        api_version: crate::API_VERSION.to_owned(),
        kind: INDEX_KIND.to_owned(),
        name: name.to_owned(),
        packs,
    })
}

/// Reads an index, checking its kind and that every entry is well formed.
pub fn read_index(bytes: &[u8]) -> PackResult<Index> {
    let index: Index = serde_json::from_slice(bytes)
        .map_err(|e| PackError::Index(format!("not a pack index: {e}")))?;
    if index.kind != INDEX_KIND || index.api_version != crate::API_VERSION {
        return Err(PackError::Index(format!(
            "not a pack index (kind {:?}, apiVersion {:?})",
            index.kind, index.api_version
        )));
    }
    for pack in &index.packs {
        if pack.versions.is_empty() {
            return Err(PackError::Index(format!("{} has no versions", pack.key)));
        }
        if let Some(v) = pack
            .versions
            .iter()
            .find(|v| v.sha256.len() != 64 || !v.sha256.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(PackError::Index(format!(
                "{} {}: bad sha256",
                pack.key, v.version
            )));
        }
    }
    Ok(index)
}

/// `index.json.sig`: the registry's signature over the index's exact bytes.
pub fn sign_index(bytes: &[u8], key: &SigningKey, registry: &str) -> PackResult<Vec<u8>> {
    let signature = Signature {
        algorithm: "ed25519".into(),
        key: key.public(),
        publisher: registry.to_owned(),
        signature: STANDARD.encode(key.pair().sign(bytes).as_ref()),
    };
    serde_json::to_vec_pretty(&signature).map_err(|e| PackError::Index(e.to_string()))
}

/// Checks `index.json.sig` was made by `expected_key` over `bytes`.
pub fn verify_index(bytes: &[u8], signature: &[u8], expected_key: &str) -> PackResult<()> {
    let signature: Signature =
        serde_json::from_slice(signature).map_err(|_| PackError::BadSignature)?;
    if signature.algorithm != "ed25519" || signature.key.trim() != expected_key.trim() {
        return Err(PackError::BadSignature);
    }
    let public = crate::public_key_bytes(&signature.key)?;
    let sig = STANDARD
        .decode(&signature.signature)
        .map_err(|_| PackError::BadSignature)?;
    UnparsedPublicKey::new(&ED25519, public)
        .verify(bytes, &sig)
        .map_err(|_| PackError::BadSignature)
}

/// Checks a downloaded pack against its index entry.
pub fn check_download(version: &IndexedVersion, bytes: &[u8]) -> PackResult<()> {
    if sha256_hex(bytes).eq_ignore_ascii_case(&version.sha256) {
        Ok(())
    } else {
        Err(PackError::Index(format!(
            "the download of {} doesn't match the index's SHA-256",
            version.version
        )))
    }
}

/// Orders `1.10.0` after `1.9.0`; pre-releases (`1.0.0-rc.1`) before their
/// release.
fn semver(version: &str) -> (u64, u64, u64, bool, String) {
    let (core, pre) = version.split_once('-').unwrap_or((version, ""));
    let mut parts = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        pre.is_empty(),
        pre.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_sort_numerically_with_prereleases_first() {
        let mut v = ["1.9.0", "1.10.0", "1.10.0-rc.1", "0.2.0"];
        v.sort_by_key(|x| std::cmp::Reverse(semver(x)));
        assert_eq!(v, ["1.10.0", "1.10.0-rc.1", "1.9.0", "0.2.0"]);
    }
}
