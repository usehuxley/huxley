//! Shared domain types for Huxley.
//!
//! This crate has no I/O. It holds identifiers, pagination primitives, and
//! other types every other crate depends on.

pub mod calendar;
pub mod context;
pub mod id;
pub mod pagination;

pub use context::{Actor, RequestContext, Via};
pub use id::new_id;
pub use pagination::{Cursor, CursorError, Page, PageRequest};
