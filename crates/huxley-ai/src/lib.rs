//! AI providers, bring your own key (design doc §10.1, ADR-031).

pub mod chat;
pub mod provider;
pub mod store;

pub use provider::{AiError, Completion, Endpoint, ProviderKind, Request, complete};
pub use store::{Ai, Purpose};
