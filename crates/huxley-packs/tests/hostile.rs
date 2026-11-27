//! Hostile packs: random bytes, damaged zips and zip bombs must be refused
//! cleanly and cheaply. Packs come from uploads and registries.

use std::io::Write as _;

use huxley_packs::{Pack, PackError};
use proptest::prelude::*;

/// A zip whose one entry inflates from about 200 KB to 200 MB of zeros.
fn zip_bomb() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    zip.start_file(
        "manifest.json",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )
    .unwrap();
    let chunk = vec![0u8; 1024 * 1024];
    for _ in 0..200 {
        zip.write_all(&chunk).unwrap();
    }
    zip.finish().unwrap();
    out.into_inner()
}

#[test]
fn zip_bombs_are_refused_without_inflating() {
    let bomb = zip_bomb();
    assert!(
        bomb.len() < 2 * 1024 * 1024,
        "the bomb is small: {} bytes",
        bomb.len()
    );
    let started = std::time::Instant::now();
    assert!(matches!(Pack::read(&bomb), Err(PackError::TooLarge)));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_bytes_are_refused_cleanly(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        prop_assert!(Pack::read(&bytes).is_err());
    }

    /// A real pack with a byte flipped anywhere is refused, never accepted
    /// changed, and never panics.
    #[test]
    fn a_damaged_pack_is_refused(at in any::<prop::sample::Index>(), bit in 0u8..8) {
        let mut bytes = huxley_packs::builtin::bytes("huxley/slack").unwrap().to_vec();
        let i = at.index(bytes.len());
        bytes[i] ^= 1 << bit;
        if let Ok(pack) = Pack::read(&bytes) {
            // Flips in zip padding or unused header fields can be harmless;
            // the content must then be exactly the original.
            let original = Pack::read(huxley_packs::builtin::bytes("huxley/slack").unwrap()).unwrap();
            prop_assert_eq!(pack.files(), original.files());
            prop_assert_eq!(pack.manifest, original.manifest);
        }
    }
}
