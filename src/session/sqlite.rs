//! SQLite-backed session store (feature `sqlite-sessions`).

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::error::{Error, Result};
use crate::llm::ChatMessage;
use crate::session::SessionStore;

/// Persists conversation history in a SQLite database (bundled, no external
/// service required). Safe to share across an application via `Arc`.
///
/// Messages are stored as JSON, so tool calls and future message fields
/// round-trip losslessly.
///
/// ```no_run
/// use corrosive_agents::session::SqliteSessionStore;
///
/// # fn run() -> corrosive_agents::Result<()> {
/// let store = SqliteSessionStore::open("sessions.db")?; // or ::in_memory()
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct SqliteSessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for SqliteSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSessionStore").finish_non_exhaustive()
    }
}

fn db_err(e: rusqlite::Error) -> Error {
    Error::Config(format!("sqlite session store: {e}"))
}

impl SqliteSessionStore {
    /// Open (or create) a session database at `path`.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::from_connection(Connection::open(path).map_err(db_err)?)
    }

    /// Create a private in-memory database (useful for tests).
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(db_err)?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                session_id TEXT NOT NULL,
                seq        INTEGER NOT NULL,
                message    TEXT NOT NULL,
                PRIMARY KEY (session_id, seq)
            );",
        )
        .map_err(db_err)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run a blocking database operation on the Tokio blocking pool.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> std::result::Result<T, rusqlite::Error> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("sqlite mutex poisoned");
            f(&conn).map_err(db_err)
        })
        .await
        .map_err(|e| Error::Config(format!("sqlite task join error: {e}")))?
    }
}

#[async_trait::async_trait]
impl SessionStore for SqliteSessionStore {
    async fn load(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let session_id = session_id.to_string();
        let rows = self
            .with_conn(move |conn| {
                let mut statement = conn
                    .prepare("SELECT message FROM messages WHERE session_id = ?1 ORDER BY seq")?;
                let rows = statement
                    .query_map(params![session_id], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;

        rows.iter()
            .map(|json| serde_json::from_str(json).map_err(Error::from))
            .collect()
    }

    async fn append(&self, session_id: &str, messages: &[ChatMessage]) -> Result<()> {
        let session_id = session_id.to_string();
        let rows: Vec<String> = messages
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<_, _>>()?;
        self.with_conn(move |conn| {
            for message in rows {
                conn.execute(
                    "INSERT INTO messages (session_id, seq, message)
                     VALUES (
                         ?1,
                         (SELECT COALESCE(MAX(seq), -1) + 1 FROM messages WHERE session_id = ?1),
                         ?2
                     )",
                    params![session_id, message],
                )?;
            }
            Ok(())
        })
        .await
    }

    async fn clear(&self, session_id: &str) -> Result<()> {
        let session_id = session_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params![session_id],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        self.with_conn(|conn| {
            let mut statement =
                conn.prepare("SELECT DISTINCT session_id FROM messages ORDER BY session_id")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    #[tokio::test]
    async fn roundtrip_in_memory() {
        let store = SqliteSessionStore::in_memory().unwrap();
        store
            .append(
                "s1",
                &[ChatMessage::user("q1"), ChatMessage::assistant("a1")],
            )
            .await
            .unwrap();
        store
            .append("s2", &[ChatMessage::user("other")])
            .await
            .unwrap();
        store
            .append("s1", &[ChatMessage::user("q2")])
            .await
            .unwrap();

        let history = store.load("s1").await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "q1");
        assert_eq!(history[2].content, "q2");
        assert!(matches!(history[1].role, Role::Assistant));

        assert_eq!(store.list_sessions().await.unwrap(), vec!["s1", "s2"]);

        store.clear("s1").await.unwrap();
        assert!(store.load("s1").await.unwrap().is_empty());
        assert_eq!(store.list_sessions().await.unwrap(), vec!["s2"]);
    }

    #[tokio::test]
    async fn tool_messages_roundtrip_losslessly() {
        use crate::llm::ToolCall;
        let store = SqliteSessionStore::in_memory().unwrap();
        let call = ToolCall {
            id: "call-1".into(),
            name: "lookup".into(),
            arguments: serde_json::json!({ "q": "rust" }),
        };
        store
            .append(
                "s",
                &[
                    ChatMessage::assistant_tool_calls("", vec![call.clone()]),
                    ChatMessage::tool_result("call-1", r#"{"answer":42}"#),
                ],
            )
            .await
            .unwrap();

        let history = store.load("s").await.unwrap();
        assert_eq!(history[0].tool_calls.as_ref().unwrap()[0], call);
        assert_eq!(history[1].tool_call_id.as_deref(), Some("call-1"));
    }

    #[tokio::test]
    async fn persists_across_handles_to_same_file() {
        let dir = std::env::temp_dir().join(format!("corrosive-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("sessions.db");

        {
            let store = SqliteSessionStore::open(&db).unwrap();
            store
                .append("persist", &[ChatMessage::user("kept")])
                .await
                .unwrap();
        }
        let reopened = SqliteSessionStore::open(&db).unwrap();
        let history = reopened.load("persist").await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "kept");

        std::fs::remove_dir_all(&dir).ok();
    }
}
