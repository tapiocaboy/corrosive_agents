//! The [`Agent`] runtime: chat sessions, skills, MCP connections, and RAG.

use std::collections::HashMap;
use std::sync::Arc;

use async_stream::try_stream;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::agent::{AgentBuilder, AgentManifest, Capability};
use crate::error::{Error, Result};
use crate::identity::AgentIdentity;
use crate::llm::{ChatMessage, ChatRequest, EmbeddingProvider, LlmProvider, StreamChunk};
use crate::mcp::{McpClient, McpTool};
use crate::skills::SkillRegistry;
use crate::vector::{Document, SearchResult, VectorStore};

/// Public, serializable snapshot of an agent — what `GET /agent` returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent name.
    pub name: String,
    /// Agent version.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Declared capabilities.
    pub capabilities: Vec<Capability>,
    /// Names of registered skills.
    pub skills: Vec<String>,
    /// Default model id, when configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Base64 Ed25519 public key, when the agent has an identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// Whether the manifest carries a signature.
    pub signed: bool,
}

/// A runnable, verifiable AI agent.
///
/// Construct with [`Agent::builder`]; see the crate-level docs for a
/// quickstart. `Agent` is designed to be shared: wrap it in an
/// [`Arc`] to serve it over REST/WebSocket/gRPC.
pub struct Agent {
    pub(crate) manifest: AgentManifest,
    pub(crate) identity: Option<AgentIdentity>,
    pub(crate) llm: Option<Arc<dyn LlmProvider>>,
    pub(crate) embeddings: Option<Arc<dyn EmbeddingProvider>>,
    pub(crate) vector_store: Option<Arc<dyn VectorStore>>,
    pub(crate) skills: SkillRegistry,
    pub(crate) mcp_clients: RwLock<HashMap<String, Arc<McpClient>>>,
    pub(crate) sessions: Arc<RwLock<HashMap<String, Vec<ChatMessage>>>>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("name", &self.manifest.name)
            .field("version", &self.manifest.version)
            .field("skills", &self.skills)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Start building an agent (Builder pattern).
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// The agent's name.
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// The agent's version.
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    /// The (possibly signed) manifest.
    pub fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    /// The agent's identity, when it has one.
    pub fn identity(&self) -> Option<&AgentIdentity> {
        self.identity.as_ref()
    }

    /// Base64 public key, when the agent has an identity.
    pub fn public_key(&self) -> Option<String> {
        self.identity.as_ref().map(AgentIdentity::public_key_base64)
    }

    /// Verify this agent's own manifest signature.
    pub fn verify(&self) -> Result<()> {
        self.manifest.verify()
    }

    /// A serializable snapshot of the agent.
    pub fn info(&self) -> AgentInfo {
        let mut skills: Vec<String> = self
            .skills
            .list()
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        skills.sort();
        AgentInfo {
            name: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            description: self.manifest.description.clone(),
            capabilities: self.manifest.capabilities.clone(),
            skills,
            model: self.manifest.model.clone(),
            public_key: self.manifest.public_key.clone(),
            signed: self.manifest.signature.is_some(),
        }
    }

    /// The names of enabled capabilities.
    pub fn active_capabilities(&self) -> Vec<&Capability> {
        self.manifest
            .capabilities
            .iter()
            .filter(|c| c.enabled)
            .collect()
    }

    /// The skill registry.
    pub fn skills(&self) -> &SkillRegistry {
        &self.skills
    }

    /// Execute a registered skill by name.
    pub async fn execute_skill(&self, name: &str, input: Value) -> Result<Value> {
        self.skills.execute(name, input).await
    }

    fn require_llm(&self) -> Result<Arc<dyn LlmProvider>> {
        self.llm.clone().ok_or(Error::NotConfigured("LLM provider"))
    }

    /// Build the message list for a turn: system prompt + session history +
    /// the new user message. Does not mutate the session.
    async fn conversation(&self, session_id: &str, message: &str) -> Vec<ChatMessage> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &self.manifest.system_prompt {
            messages.push(ChatMessage::system(system_prompt));
        }
        if let Some(history) = self.sessions.read().await.get(session_id) {
            messages.extend(history.iter().cloned());
        }
        messages.push(ChatMessage::user(message));
        messages
    }

    fn request_for(&self, messages: Vec<ChatMessage>) -> ChatRequest {
        let mut request = ChatRequest::new(messages);
        request.model = self.manifest.model.clone();
        request
    }

    async fn record_turn(
        sessions: &RwLock<HashMap<String, Vec<ChatMessage>>>,
        session_id: &str,
        user: &str,
        assistant: &str,
    ) {
        let mut sessions = sessions.write().await;
        let history = sessions.entry(session_id.to_string()).or_default();
        history.push(ChatMessage::user(user));
        history.push(ChatMessage::assistant(assistant));
    }

    /// Send a message in the given session and return the assistant's reply.
    ///
    /// Conversation history is kept per `session_id`; the manifest's system
    /// prompt (if any) is prepended to every turn.
    pub async fn chat(&self, session_id: &str, message: impl AsRef<str>) -> Result<String> {
        let message = message.as_ref();
        let llm = self.require_llm()?;
        let request = self.request_for(self.conversation(session_id, message).await);
        let response = llm.chat(request).await?;
        Self::record_turn(&self.sessions, session_id, message, &response.content).await;
        Ok(response.content)
    }

    /// Streaming variant of [`chat`](Self::chat): yields incremental
    /// [`StreamChunk`]s and records the full reply in the session once the
    /// stream completes.
    pub async fn chat_stream(
        &self,
        session_id: &str,
        message: impl AsRef<str>,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>> {
        let message = message.as_ref().to_string();
        let llm = self.require_llm()?;
        let request = self.request_for(self.conversation(session_id, &message).await);
        let mut inner = llm.chat_stream(request).await?;

        let sessions = Arc::clone(&self.sessions);
        let session_id = session_id.to_string();
        let stream = try_stream! {
            let mut reply = String::new();
            while let Some(chunk) = inner.next().await {
                let chunk = chunk?;
                if chunk.done {
                    let mut sessions = sessions.write().await;
                    let history = sessions.entry(session_id.clone()).or_default();
                    history.push(ChatMessage::user(message.clone()));
                    history.push(ChatMessage::assistant(reply.clone()));
                    yield chunk;
                    break;
                }
                reply.push_str(&chunk.delta);
                yield chunk;
            }
        };
        Ok(Box::pin(stream))
    }

    /// Drop the conversation history for a session.
    pub async fn clear_session(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
    }

    /// Connect to every MCP server declared in the manifest.
    /// Returns the names of the servers that were connected.
    pub async fn connect_mcp_servers(&self) -> Result<Vec<String>> {
        let mut connected = Vec::new();
        for config in &self.manifest.mcp_servers {
            let client = McpClient::connect(config).await?;
            self.mcp_clients
                .write()
                .await
                .insert(config.name.clone(), Arc::new(client));
            connected.push(config.name.clone());
        }
        Ok(connected)
    }

    /// Get a connected MCP client by its configured name.
    pub async fn mcp_client(&self, name: &str) -> Option<Arc<McpClient>> {
        self.mcp_clients.read().await.get(name).cloned()
    }

    /// List the tools of a connected MCP server.
    pub async fn mcp_tools(&self, server: &str) -> Result<Vec<McpTool>> {
        let client = self
            .mcp_client(server)
            .await
            .ok_or_else(|| Error::Mcp(format!("MCP server '{server}' is not connected")))?;
        client.list_tools().await
    }

    /// Call a tool on a connected MCP server.
    pub async fn call_mcp_tool(&self, server: &str, tool: &str, arguments: Value) -> Result<Value> {
        let client = self
            .mcp_client(server)
            .await
            .ok_or_else(|| Error::Mcp(format!("MCP server '{server}' is not connected")))?;
        client.call_tool(tool, arguments).await
    }

    /// Embed `text` and store it in the vector store. Returns the generated
    /// document id. Requires an embedding provider and a vector store.
    pub async fn remember(&self, text: &str, metadata: Value) -> Result<String> {
        let embeddings = self
            .embeddings
            .clone()
            .ok_or(Error::NotConfigured("embedding provider"))?;
        let store = self
            .vector_store
            .clone()
            .ok_or(Error::NotConfigured("vector store"))?;

        let mut vectors = embeddings.embed_documents(&[text.to_string()]).await?;
        let vector = vectors
            .pop()
            .ok_or_else(|| Error::Llm("embedding provider returned no vectors".into()))?;
        let id = uuid::Uuid::new_v4().to_string();
        store
            .upsert(vec![Document::new(&id, vector)
                .with_text(text)
                .with_metadata(metadata)])
            .await?;
        Ok(id)
    }

    /// Embed `query` and return the `top_k` most similar remembered documents.
    pub async fn recall(&self, query: &str, top_k: usize) -> Result<Vec<SearchResult>> {
        let embeddings = self
            .embeddings
            .clone()
            .ok_or(Error::NotConfigured("embedding provider"))?;
        let store = self
            .vector_store
            .clone()
            .ok_or(Error::NotConfigured("vector store"))?;

        let vector = embeddings.embed_query(query).await?;
        store.search(vector, top_k).await
    }

    /// The configured vector store, when present.
    pub fn vector_store(&self) -> Option<Arc<dyn VectorStore>> {
        self.vector_store.clone()
    }
}
