//! Built-in packs: shipped inside the binary, so trusted by construction.
//! A pack whose manifest and files are exactly a built-in one's is
//! recognised as built in wherever it comes from.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::{Manifest, Pack};

struct Source {
    manifest: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const SOURCES: &[Source] = &[
    Source {
        manifest: include_str!("../builtin/servicedesk-email/pack.yaml"),
        files: &[
            (
                "workflows/email-to-ticket.yaml",
                include_str!("../builtin/servicedesk-email/workflows/email-to-ticket.yaml"),
            ),
            (
                "workflows/poll-mailboxes.yaml",
                include_str!("../builtin/servicedesk-email/workflows/poll-mailboxes.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/msgraph/pack.yaml"),
        files: &[
            (
                "integrations/msgraph.yaml",
                include_str!("../builtin/msgraph/integrations/msgraph.yaml"),
            ),
            (
                "integrations/msgraph.overlay.json",
                include_str!("../builtin/msgraph/integrations/msgraph.overlay.json"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/slack/pack.yaml"),
        files: &[
            (
                "integrations/slack.yaml",
                include_str!("../builtin/slack/integrations/slack.yaml"),
            ),
            (
                "integrations/slack.overlay.json",
                include_str!("../builtin/slack/integrations/slack.overlay.json"),
            ),
        ],
    },
];

/// Each built-in pack, and its bytes.
fn all() -> &'static [(Pack, Vec<u8>)] {
    static PACKS: OnceLock<Vec<(Pack, Vec<u8>)>> = OnceLock::new();
    PACKS.get_or_init(|| {
        SOURCES
            .iter()
            .map(|source| {
                let manifest: Manifest =
                    serde_saphyr::from_str(source.manifest).expect("built-in manifests parse");
                let files: BTreeMap<String, Vec<u8>> = source
                    .files
                    .iter()
                    .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
                    .collect();
                let bytes = crate::build(manifest, &files, None).expect("built-in packs build");
                let mut pack = Pack::read_unrecognised(&bytes).expect("built-in packs read");
                pack.builtin = true;
                (pack, bytes)
            })
            .collect()
    })
}

/// Every built-in pack.
pub fn list() -> Vec<Pack> {
    all().iter().map(|(pack, _)| pack.clone()).collect()
}

/// Built-in pack `key` (e.g. `huxley/msgraph`), as a `.hxpack`.
pub fn bytes(key: &str) -> Option<&'static [u8]> {
    all()
        .iter()
        .find(|(pack, _)| pack.manifest.key == key)
        .map(|(_, bytes)| bytes.as_slice())
}

/// The built-in pack that adds integration `key`.
pub fn for_integration(key: &str) -> Option<Pack> {
    list()
        .into_iter()
        .find(|p| p.manifest.contents.integrations.iter().any(|k| k == key))
}

/// Whether `pack` is exactly a built-in one.
pub(crate) fn is_builtin(pack: &Pack) -> bool {
    all()
        .iter()
        .any(|(b, _)| b.manifest == pack.manifest && b.files == pack.files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Trust;

    #[test]
    fn built_ins_are_recognised_wherever_they_come_from() {
        assert_eq!(list().len(), SOURCES.len());
        let bytes = bytes("huxley/msgraph").unwrap();
        let pack = Pack::read(bytes).unwrap();
        assert_eq!(
            pack.trust(&[]),
            Trust::BuiltIn {
                publisher: "Huxley".into()
            }
        );
        assert_eq!(
            for_integration("slack").unwrap().manifest.key,
            "huxley/slack"
        );
        // A changed copy is just an unsigned pack.
        let mut files = pack.files().clone();
        files.insert("integrations/msgraph.overlay.json".into(), b"{}".to_vec());
        let changed = crate::build(pack.manifest.clone(), &files, None).unwrap();
        assert_eq!(Pack::read(&changed).unwrap().trust(&[]), Trust::Unsigned);
    }
}
