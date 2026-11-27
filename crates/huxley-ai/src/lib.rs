//! AI providers, bring your own key (design doc §10.1, ADR-031).

pub mod chat;
pub mod embed;
pub mod provider;
pub mod store;
mod stream;

pub use provider::{AiError, Attachment, Completion, Endpoint, ProviderKind, Request, complete};
pub use store::{Ai, Purpose};
