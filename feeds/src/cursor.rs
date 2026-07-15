//! The resumable position a poll source persists through the store sink.

use anyhow::Result;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A source's opaque resume position, carried as JSON so the framework never
/// needs to know its shape. A CEX feed stores `{ "next_start": <epoch> }`, an
/// RPC feed a signature or slot; each source serializes and reads back its
/// own type via [`Cursor::new`] / [`Cursor::get`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(serde_json::Value);

impl Cursor {
    /// Serialize a source's typed cursor into the opaque form.
    pub fn new<T: Serialize>(value: &T) -> Result<Self> {
        Ok(Self(serde_json::to_value(value)?))
    }

    /// Wrap an already-decoded JSON value (e.g. one loaded from the store).
    pub fn from_json(value: serde_json::Value) -> Self {
        Self(value)
    }

    /// Deserialize back into the source's typed cursor.
    pub fn get<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_value(self.0.clone())?)
    }

    /// The underlying JSON, for persistence.
    pub fn as_json(&self) -> &serde_json::Value {
        &self.0
    }

    /// Consume into the underlying JSON.
    pub fn into_json(self) -> serde_json::Value {
        self.0
    }
}

/// The framework-owned durable position store (the `feed_cursors` table). The
/// store sink saves after each committed batch; a poll source loads at startup
/// to resume. Keyed by [`crate::Source::name`]. A forward-only (live) consumer
/// needs no cursor and never touches this.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// The last saved cursor for `feed`, or `None` if it has never run.
    async fn load(&self, feed: &str) -> Result<Option<Cursor>>;
    /// Persist `feed`'s cursor, overwriting any previous position.
    async fn save(&self, feed: &str, cursor: &Cursor) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct CexCursor {
        next_start: u64,
    }

    #[test]
    fn round_trips_a_typed_cursor() {
        let original = CexCursor {
            next_start: 1_700_000_000,
        };
        let cursor = Cursor::new(&original).unwrap();
        assert_eq!(cursor.get::<CexCursor>().unwrap(), original);
    }

    #[test]
    fn preserves_json_across_wrap() {
        let cursor = Cursor::new(&CexCursor { next_start: 7 }).unwrap();
        let reloaded = Cursor::from_json(cursor.as_json().clone());
        assert_eq!(cursor, reloaded);
    }
}
