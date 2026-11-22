//! Cursor pagination (see `docs/api-conventions.md`).
//!
//! Cursors are opaque to clients: a URL-safe base64 encoding of a small JSON
//! object holding the sort key of the last item returned. Clients must never
//! construct or parse them.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Default page size when the client does not ask for one.
pub const DEFAULT_LIMIT: u32 = 50;
/// Hard upper bound on page size.
pub const MAX_LIMIT: u32 = 200;

/// An opaque pagination cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(String);

#[derive(Debug, thiserror::Error)]
pub enum CursorError {
    #[error("cursor is not valid base64")]
    Encoding,
    #[error("cursor payload is malformed")]
    Payload,
}

impl Cursor {
    /// Encodes a sort key into an opaque cursor.
    pub fn encode<T: Serialize>(key: &T) -> Self {
        // Serializing plain data structs cannot fail.
        let json = serde_json::to_vec(key).expect("cursor key must serialize");
        Self(URL_SAFE_NO_PAD.encode(json))
    }

    /// Decodes an opaque cursor back into its sort key.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, CursorError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(&self.0)
            .map_err(|_| CursorError::Encoding)?;
        serde_json::from_slice(&bytes).map_err(|_| CursorError::Payload)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for Cursor {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Pagination parameters from a list request.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PageRequest {
    pub limit: Option<u32>,
    pub cursor: Option<Cursor>,
}

impl PageRequest {
    /// The effective page size, clamped to `1..=MAX_LIMIT`.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
}

/// A page of results. Serializes as `{ "items": [...], "nextCursor": ... }`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Cursor for the next page, or `None` if this is the last page.
    pub next_cursor: Option<Cursor>,
}

impl<T> Page<T> {
    /// Builds a page from `limit + 1` fetched rows. The extra row is used only
    /// to detect whether another page exists, then dropped.
    pub fn from_overfetch<K: Serialize>(
        mut rows: Vec<T>,
        limit: u32,
        key_of: impl Fn(&T) -> K,
    ) -> Self {
        let limit = limit as usize;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = if has_more {
            rows.last().map(|last| Cursor::encode(&key_of(last)))
        } else {
            None
        };
        Self {
            items: rows,
            next_cursor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Key {
        id: u32,
    }

    #[test]
    fn cursor_round_trips() {
        let cursor = Cursor::encode(&Key { id: 42 });
        assert_eq!(cursor.decode::<Key>().unwrap(), Key { id: 42 });
    }

    #[test]
    fn garbage_cursor_is_rejected() {
        let cursor = Cursor::from("not base64 !!".to_owned());
        assert!(matches!(cursor.decode::<Key>(), Err(CursorError::Encoding)));
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(PageRequest::default().limit(), DEFAULT_LIMIT);
        let big = PageRequest {
            limit: Some(10_000),
            cursor: None,
        };
        assert_eq!(big.limit(), MAX_LIMIT);
        let zero = PageRequest {
            limit: Some(0),
            cursor: None,
        };
        assert_eq!(zero.limit(), 1);
    }

    #[test]
    fn page_serializes_camel_case() {
        let page = Page::from_overfetch(vec![1], 5, |n| Key { id: *n });
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "items": [1], "nextCursor": null })
        );
    }

    #[test]
    fn overfetch_sets_next_cursor_only_when_more_rows_exist() {
        let page = Page::from_overfetch(vec![1, 2, 3], 2, |n| Key { id: *n });
        assert_eq!(page.items, vec![1, 2]);
        assert_eq!(
            page.next_cursor.unwrap().decode::<Key>().unwrap(),
            Key { id: 2 }
        );

        let last = Page::from_overfetch(vec![1, 2], 2, |n| Key { id: *n });
        assert!(last.next_cursor.is_none());
    }
}
