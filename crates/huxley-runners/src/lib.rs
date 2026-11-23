//! Script runners (design doc §6.9, ADR-025).

pub mod ca;
pub mod gateway;
pub mod registry;

/// The generated runner protocol.
#[allow(clippy::pedantic, clippy::nursery, missing_docs)]
pub mod proto {
    tonic::include_proto!("huxley.runner.v1");
}

/// Picks rustls's ring provider for this process (several are compiled
/// in, so rustls can't choose by itself). Safe to call more than once.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
