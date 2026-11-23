//! OIDC single sign-on (design doc §11.2).
//!
//! - [`providers`]: identity provider configuration (with presets for Entra,
//!   Google, Okta, Authentik, and Keycloak) and claim → role mappings.
//! - [`SsoService`]: the authorization code flow with PKCE and a nonce,
//!   linking to or creating Huxley users, syncing role assignments from
//!   claims, and starting the session.
//!
//! Account safety rules:
//! - An identity is keyed by (provider, subject). A provider can only sign in
//!   addresses in its `allowed_domains`.
//! - A first sign-in links to an existing user with the same email, or
//!   creates one when JIT provisioning is on, only if the email is verified
//!   (`email_verified`, or `trust_email` for providers that don't send it).
//! - Mapped roles never include Owner. Sign-in adds and removes only the
//!   assignments a provider created, never ones made by hand.

mod login;
pub mod providers;

use huxley_auth::AuthError;
use huxley_crypto::CryptoError;
use huxley_db::DbError;

pub use login::{SignIn, SsoService, StartedSignIn};
pub use providers::{MfaTrust, Preset, Provider, ProviderButton, ProviderInput, RoleMapping};

#[derive(Debug, thiserror::Error)]
pub enum SsoError {
    #[error("{field}: {message}")]
    Invalid {
        field: &'static str,
        message: &'static str,
    },
    #[error("identity provider not found")]
    ProviderNotFound,
    #[error("single sign-on needs auth.public_url for its redirect address")]
    NotConfigured,
    #[error("the sign-in attempt expired or was already used")]
    InvalidState,
    /// The provider answered with an error, or its tokens didn't verify.
    #[error("the identity provider's response could not be verified: {0}")]
    Provider(String),
    #[error("the provider didn't return an email address")]
    MissingEmail,
    #[error("the provider didn't confirm the email address is verified")]
    UnverifiedEmail,
    #[error("this provider can't sign in addresses at that domain")]
    DomainNotAllowed,
    #[error("there's no Huxley account for this identity")]
    NoAccount,
    #[error("the account is disabled")]
    Disabled,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for SsoError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(error))
    }
}

impl SsoError {
    /// A short reason for audit records and the sign-in page.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "invalid",
            Self::ProviderNotFound => "unknown_provider",
            Self::NotConfigured => "not_configured",
            Self::InvalidState => "invalid_state",
            Self::Provider(_) => "provider_error",
            Self::MissingEmail => "missing_email",
            Self::UnverifiedEmail => "unverified_email",
            Self::DomainNotAllowed => "domain_not_allowed",
            Self::NoAccount => "no_account",
            Self::Disabled => "disabled",
            Self::Auth(_) | Self::Crypto(_) | Self::Db(_) => "internal",
        }
    }
}

pub type SsoResult<T> = Result<T, SsoError>;
