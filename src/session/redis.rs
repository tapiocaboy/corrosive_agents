//! Redis-backed session store (feature `redis-sessions`).

use redis::aio::ConnectionManager;
use redis::AsyncCommands;

use crate::error::{Error, Result};
use crate::llm::ChatMessage;
use crate::session::SessionStore;

fn redis_err(e: redis::RedisError) -> Error {
    Error::Config(format!("redis session store: {e}"))
}

/// Persists conversation history in Redis — the right choice when several
/// agent instances must share sessions. Each session is a Redis list of
/// JSON-encoded messages under `{prefix}{session_id}`.
///
/// ```no_run
/// use corrosive_agents::session::RedisSessionStore;
///
/// # async fn run() -> corrosive_agents::Result<()> {
/// let store = RedisSessionStore::connect("redis://127.0.0.1/")
///     .await?
///     .with_ttl(std::time::Duration::from_secs(24 * 3600));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct RedisSessionStore {
    manager: ConnectionManager,
    prefix: String,
    ttl_seconds: Option<u64>,
}

impl std::fmt::Debug for RedisSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisSessionStore")
            .field("prefix", &self.prefix)
            .field("ttl_seconds", &self.ttl_seconds)
            .finish_non_exhaustive()
    }
}

impl RedisSessionStore {
    /// Connect to Redis at `url` (e.g. `redis://127.0.0.1/`). The underlying
    /// connection reconnects automatically.
    pub async fn connect(url: &str) -> Result<Self> {
        let client = redis::Client::open(url).map_err(redis_err)?;
        let manager = ConnectionManager::new(client).await.map_err(redis_err)?;
        Ok(Self {
            manager,
            prefix: "corrosive:session:".to_string(),
            ttl_seconds: None,
        })
    }

    /// Change the key prefix (default `corrosive:session:`).
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Expire sessions after `ttl` of inactivity (the timer resets on every
    /// append).
    #[must_use]
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl_seconds = Some(ttl.as_secs());
        self
    }

    fn key(&self, session_id: &str) -> String {
        format!("{}{}", self.prefix, session_id)
    }
}

#[async_trait::async_trait]
impl SessionStore for RedisSessionStore {
    async fn load(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let mut conn = self.manager.clone();
        let raw: Vec<String> = conn
            .lrange(self.key(session_id), 0, -1)
            .await
            .map_err(redis_err)?;
        raw.iter()
            .map(|json| serde_json::from_str(json).map_err(Error::from))
            .collect()
    }

    async fn append(&self, session_id: &str, messages: &[ChatMessage]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let key = self.key(session_id);
        let encoded: Vec<String> = messages
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<_, _>>()?;
        let mut conn = self.manager.clone();
        let _: () = conn.rpush(&key, encoded).await.map_err(redis_err)?;
        if let Some(ttl) = self.ttl_seconds {
            let _: () = conn.expire(&key, ttl as i64).await.map_err(redis_err)?;
        }
        Ok(())
    }

    async fn clear(&self, session_id: &str) -> Result<()> {
        let mut conn = self.manager.clone();
        let _: () = conn.del(self.key(session_id)).await.map_err(redis_err)?;
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        let mut conn = self.manager.clone();
        let keys: Vec<String> = conn
            .keys(format!("{}*", self.prefix))
            .await
            .map_err(redis_err)?;
        Ok(keys
            .into_iter()
            .filter_map(|k| k.strip_prefix(&self.prefix).map(String::from))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full roundtrip against a real Redis. Skipped unless `REDIS_URL` is set
    /// (e.g. `REDIS_URL=redis://127.0.0.1/ cargo test --features redis-sessions`).
    #[tokio::test]
    async fn roundtrip_against_real_redis() {
        let Ok(url) = std::env::var("REDIS_URL") else {
            eprintln!("REDIS_URL not set — skipping Redis integration test");
            return;
        };
        let store = RedisSessionStore::connect(&url)
            .await
            .unwrap()
            .with_prefix(format!("corrosive-test:{}:", uuid::Uuid::new_v4()));

        store
            .append("s1", &[ChatMessage::user("q"), ChatMessage::assistant("a")])
            .await
            .unwrap();
        let history = store.load("s1").await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(store.list_sessions().await.unwrap(), vec!["s1"]);

        store.clear("s1").await.unwrap();
        assert!(store.load("s1").await.unwrap().is_empty());
    }
}
