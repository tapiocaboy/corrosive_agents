//! Default in-process session store.

use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::error::Result;
use crate::llm::ChatMessage;
use crate::session::SessionStore;

/// Keeps conversation history in process memory. This is the default store;
/// history is lost when the process exits.
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, Vec<ChatMessage>>>,
}

impl InMemorySessionStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl SessionStore for InMemorySessionStore {
    async fn load(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        Ok(self
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn append(&self, session_id: &str, messages: &[ChatMessage]) -> Result<()> {
        self.sessions
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .extend_from_slice(messages);
        Ok(())
    }

    async fn clear(&self, session_id: &str) -> Result<()> {
        self.sessions.write().await.remove(session_id);
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<String>> {
        Ok(self.sessions.read().await.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let store = InMemorySessionStore::new();
        store
            .append(
                "s1",
                &[ChatMessage::user("hi"), ChatMessage::assistant("hello")],
            )
            .await
            .unwrap();
        store
            .append("s1", &[ChatMessage::user("more")])
            .await
            .unwrap();

        let history = store.load("s1").await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[2].content, "more");
        assert_eq!(store.list_sessions().await.unwrap(), vec!["s1"]);

        store.clear("s1").await.unwrap();
        assert!(store.load("s1").await.unwrap().is_empty());
    }
}
