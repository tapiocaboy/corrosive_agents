//! Session stores: pluggable persistence for conversation history.
//!
//! Every [`Agent`](crate::agent::Agent) keeps per-session chat history in a
//! [`SessionStore`]. The default is [`InMemorySessionStore`] (history lives
//! and dies with the process). For persistence across restarts, enable a
//! backend feature and pass a store to the builder:
//!
//! - [`SqliteSessionStore`] (feature `sqlite-sessions`) — single-file or
//!   in-memory SQLite database, no external service needed.
//! - [`RedisSessionStore`] (feature `redis-sessions`) — shared store for
//!   multi-instance deployments, with optional TTL expiry.
//!
//! Implement the trait yourself for any other backend (Postgres, DynamoDB, …).

mod memory;
#[cfg(feature = "redis-sessions")]
mod redis;
#[cfg(feature = "sqlite-sessions")]
mod sqlite;

pub use memory::InMemorySessionStore;
#[cfg(feature = "redis-sessions")]
#[cfg_attr(docsrs, doc(cfg(feature = "redis-sessions")))]
pub use redis::RedisSessionStore;
#[cfg(feature = "sqlite-sessions")]
#[cfg_attr(docsrs, doc(cfg(feature = "sqlite-sessions")))]
pub use sqlite::SqliteSessionStore;

use crate::error::Result;
use crate::llm::ChatMessage;

/// Persistence for per-session conversation history.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Load the full message history of a session (empty if unknown).
    async fn load(&self, session_id: &str) -> Result<Vec<ChatMessage>>;

    /// Append messages to a session, creating it if needed.
    async fn append(&self, session_id: &str, messages: &[ChatMessage]) -> Result<()>;

    /// Delete a session and its history.
    async fn clear(&self, session_id: &str) -> Result<()>;

    /// Ids of all known sessions.
    async fn list_sessions(&self) -> Result<Vec<String>>;
}
