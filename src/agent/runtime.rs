//! The [`Agent`] runtime: chat sessions, skills, MCP connections, and RAG.

use std::collections::HashMap;
use std::sync::Arc;

use async_stream::try_stream;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::a2a::RemoteAgent;
use crate::agent::{AgentBuilder, AgentManifest, Capability};
use crate::error::{Error, Result};
use crate::identity::AgentIdentity;
use crate::llm::{
    ChatMessage, ChatRequest, ChatResponse, EmbeddingProvider, LlmProvider, StreamChunk, ToolSpec,
    UsageEvent, UsageObserver, UsageSnapshot, UsageTotals,
};
use crate::mcp::{McpClient, McpTool};
use crate::session::SessionStore;
use crate::skills::{SkillPolicy, SkillRegistry};
use crate::vector::{chunk_text, Document, MetadataFilter, SearchResult, VectorStore};

/// Public, serializable snapshot of an agent — what `GET /agent` returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    pub(crate) skill_policy: SkillPolicy,
    pub(crate) mcp_clients: RwLock<HashMap<String, Arc<McpClient>>>,
    pub(crate) sessions: Arc<dyn SessionStore>,
    pub(crate) peers: RwLock<HashMap<String, Arc<RemoteAgent>>>,
    pub(crate) usage_totals: Arc<UsageTotals>,
    pub(crate) usage_observer: Option<Arc<dyn UsageObserver>>,
    pub(crate) ready: std::sync::atomic::AtomicBool,
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

    /// Execute a registered skill by name, subject to the agent's
    /// [`SkillPolicy`] (allowlist, permissions, timeout). Execution runs on
    /// a separate task so a panicking skill cannot take the agent down.
    pub async fn execute_skill(&self, name: &str, input: Value) -> Result<Value> {
        let skill = self
            .skills
            .get(name)
            .ok_or_else(|| Error::SkillNotFound(name.to_string()))?;
        self.skill_policy.check(skill.as_ref())?;

        let handle = tokio::spawn(async move { skill.execute(input).await });
        let abort = handle.abort_handle();
        let outcome = match self.skill_policy.timeout() {
            Some(limit) => tokio::time::timeout(limit, handle).await.map_err(|_| {
                abort.abort(); // don't leave the runaway skill running
                Error::Skill(format!("skill '{name}' timed out after {limit:?}"))
            })?,
            None => handle.await,
        };
        outcome.map_err(|e| Error::Skill(format!("skill '{name}' panicked: {e}")))?
    }

    /// Record token usage from a completed response.
    fn record_usage(&self, session_id: &str, response: &ChatResponse) {
        if let Some(usage) = &response.usage {
            let event = UsageEvent {
                session_id: session_id.to_string(),
                model: response.model.clone(),
                usage: usage.clone(),
            };
            self.usage_totals.record(&event);
            if let Some(observer) = &self.usage_observer {
                observer.on_usage(&event);
            }
        }
    }

    /// Cumulative token usage across all sessions since the agent started.
    pub fn usage(&self) -> UsageSnapshot {
        self.usage_totals.snapshot()
    }

    fn require_llm(&self) -> Result<Arc<dyn LlmProvider>> {
        self.llm.clone().ok_or(Error::NotConfigured("LLM provider"))
    }

    /// Build the message list for a turn: system prompt + session history +
    /// the new user message. Does not mutate the session.
    async fn conversation(&self, session_id: &str, message: &str) -> Result<Vec<ChatMessage>> {
        let mut messages = Vec::new();
        if let Some(system_prompt) = &self.manifest.system_prompt {
            messages.push(ChatMessage::system(system_prompt));
        }
        messages.extend(self.sessions.load(session_id).await?);
        messages.push(ChatMessage::user(message));
        Ok(messages)
    }

    fn request_for(&self, messages: Vec<ChatMessage>) -> ChatRequest {
        let mut request = ChatRequest::new(messages);
        request.model = self.manifest.model.clone();
        request
    }

    /// Send a message in the given session and return the assistant's reply.
    ///
    /// Conversation history is kept per `session_id` in the agent's
    /// [`SessionStore`]; the manifest's system prompt (if any) is prepended
    /// to every turn.
    pub async fn chat(&self, session_id: &str, message: impl AsRef<str>) -> Result<String> {
        let message = message.as_ref();
        let llm = self.require_llm()?;
        let request = self.request_for(self.conversation(session_id, message).await?);
        let response = llm.chat(request).await?;
        self.record_usage(session_id, &response);
        self.sessions
            .append(
                session_id,
                &[
                    ChatMessage::user(message),
                    ChatMessage::assistant(&response.content),
                ],
            )
            .await?;
        Ok(response.content)
    }

    /// Like [`chat`](Self::chat), but the model may call the agent's
    /// registered skills as tools (function calling).
    ///
    /// The loop runs until the model answers with plain text (or
    /// `max_rounds` tool rounds elapse): tool calls are executed through
    /// [`execute_skill`](Self::execute_skill) — so the [`SkillPolicy`] is
    /// enforced — and their JSON results are fed back to the model. Skill
    /// failures are reported to the model as `{"error": …}` results rather
    /// than aborting the turn.
    pub async fn chat_with_tools(
        &self,
        session_id: &str,
        message: impl AsRef<str>,
        max_rounds: usize,
    ) -> Result<String> {
        let message = message.as_ref();
        let llm = self.require_llm()?;
        let tools: Vec<ToolSpec> = self
            .skills
            .list()
            .iter()
            .map(|s| ToolSpec::from_skill(s.as_ref()))
            .collect();

        let mut messages = self.conversation(session_id, message).await?;
        // Everything after (and including) the user message gets persisted.
        let mut transcript: Vec<ChatMessage> = vec![ChatMessage::user(message)];

        for _ in 0..max_rounds.max(1) {
            let mut request = self.request_for(messages.clone());
            if !tools.is_empty() {
                request.tools = Some(tools.clone());
            }
            let response = llm.chat(request).await?;
            self.record_usage(session_id, &response);

            if response.tool_calls.is_empty() {
                transcript.push(ChatMessage::assistant(&response.content));
                self.sessions.append(session_id, &transcript).await?;
                return Ok(response.content);
            }

            let assistant = ChatMessage::assistant_tool_calls(
                response.content.clone(),
                response.tool_calls.clone(),
            );
            messages.push(assistant.clone());
            transcript.push(assistant);

            for call in response.tool_calls {
                let output = match self.execute_skill(&call.name, call.arguments.clone()).await {
                    Ok(value) => value.to_string(),
                    Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                };
                let result = ChatMessage::tool_result(&call.id, output);
                messages.push(result.clone());
                transcript.push(result);
            }
        }
        Err(Error::Llm(format!(
            "tool-calling loop did not converge within {max_rounds} rounds"
        )))
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
        let request = self.request_for(self.conversation(session_id, &message).await?);
        let mut inner = llm.chat_stream(request).await?;

        let sessions = Arc::clone(&self.sessions);
        let session_id = session_id.to_string();
        let stream = try_stream! {
            let mut reply = String::new();
            while let Some(chunk) = inner.next().await {
                let chunk = chunk?;
                if chunk.done {
                    sessions
                        .append(
                            &session_id,
                            &[
                                ChatMessage::user(message.clone()),
                                ChatMessage::assistant(reply.clone()),
                            ],
                        )
                        .await?;
                    yield chunk;
                    break;
                }
                reply.push_str(&chunk.delta);
                yield chunk;
            }
        };
        Ok(Box::pin(stream))
    }

    /// The full conversation history of a session.
    pub async fn session_history(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        self.sessions.load(session_id).await
    }

    /// Ids of all sessions known to the session store.
    pub async fn list_sessions(&self) -> Result<Vec<String>> {
        self.sessions.list_sessions().await
    }

    /// Drop the conversation history for a session.
    pub async fn clear_session(&self, session_id: &str) -> Result<()> {
        self.sessions.clear(session_id).await
    }

    /// The session store backing this agent's conversation history.
    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        Arc::clone(&self.sessions)
    }

    /// Register a peer agent for delegation under a local name.
    pub async fn add_peer(&self, name: impl Into<String>, peer: RemoteAgent) {
        self.peers.write().await.insert(name.into(), Arc::new(peer));
    }

    /// Get a registered peer by name.
    pub async fn peer(&self, name: &str) -> Option<Arc<RemoteAgent>> {
        self.peers.read().await.get(name).cloned()
    }

    /// Names of all registered peers.
    pub async fn list_peers(&self) -> Vec<String> {
        self.peers.read().await.keys().cloned().collect()
    }

    async fn require_peer(&self, name: &str) -> Result<Arc<RemoteAgent>> {
        self.peer(name)
            .await
            .ok_or_else(|| Error::A2a(format!("peer '{name}' is not registered")))
    }

    /// Delegate a chat turn to a registered peer agent (A2A). If the peer was
    /// registered with a pinned key, its manifest is cryptographically
    /// verified before the first delegation.
    pub async fn delegate_chat(
        &self,
        peer: &str,
        session_id: &str,
        message: &str,
    ) -> Result<String> {
        self.require_peer(peer)
            .await?
            .chat(session_id, message)
            .await
    }

    /// Delegate a skill execution to a registered peer agent (A2A).
    pub async fn delegate_skill(&self, peer: &str, skill: &str, input: Value) -> Result<Value> {
        self.require_peer(peer)
            .await?
            .execute_skill(skill, input)
            .await
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

    /// Embed and store several texts at once (batched embedding + batched
    /// upsert). Returns the generated document ids, in input order.
    pub async fn remember_batch(&self, texts: &[String], metadata: Value) -> Result<Vec<String>> {
        let embeddings = self
            .embeddings
            .clone()
            .ok_or(Error::NotConfigured("embedding provider"))?;
        let store = self
            .vector_store
            .clone()
            .ok_or(Error::NotConfigured("vector store"))?;
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let vectors = embeddings.embed_documents(texts).await?;
        if vectors.len() != texts.len() {
            return Err(Error::Llm(format!(
                "embedding provider returned {} vectors for {} texts",
                vectors.len(),
                texts.len()
            )));
        }
        let mut ids = Vec::with_capacity(texts.len());
        let documents: Vec<Document> = texts
            .iter()
            .zip(vectors)
            .map(|(text, vector)| {
                let id = uuid::Uuid::new_v4().to_string();
                ids.push(id.clone());
                Document::new(id, vector)
                    .with_text(text)
                    .with_metadata(metadata.clone())
            })
            .collect();
        store.upsert_batched(documents, 64).await?;
        Ok(ids)
    }

    /// Chunk a long document (word-boundary chunks of `max_chars` with
    /// `overlap` characters of carried context — see
    /// [`chunk_text`](crate::vector::chunk_text)), then embed and store every
    /// chunk. Each chunk's metadata gains a `_chunk` index.
    pub async fn remember_document(
        &self,
        text: &str,
        metadata: Value,
        max_chars: usize,
        overlap: usize,
    ) -> Result<Vec<String>> {
        let chunks = chunk_text(text, max_chars, overlap);
        let mut ids = Vec::with_capacity(chunks.len());
        for (index, chunk) in chunks.iter().enumerate() {
            let mut chunk_metadata = metadata.clone();
            if let Value::Object(map) = &mut chunk_metadata {
                map.insert("_chunk".into(), Value::from(index));
            }
            ids.extend(
                self.remember_batch(std::slice::from_ref(chunk), chunk_metadata)
                    .await?,
            );
        }
        Ok(ids)
    }

    /// Embed `query` and return the `top_k` most similar remembered documents.
    pub async fn recall(&self, query: &str, top_k: usize) -> Result<Vec<SearchResult>> {
        self.recall_filtered(query, top_k, &MetadataFilter::new())
            .await
    }

    /// Like [`recall`](Self::recall), restricted to documents whose metadata
    /// matches `filter`.
    pub async fn recall_filtered(
        &self,
        query: &str,
        top_k: usize,
        filter: &MetadataFilter,
    ) -> Result<Vec<SearchResult>> {
        let embeddings = self
            .embeddings
            .clone()
            .ok_or(Error::NotConfigured("embedding provider"))?;
        let store = self
            .vector_store
            .clone()
            .ok_or(Error::NotConfigured("vector store"))?;

        let vector = embeddings.embed_query(query).await?;
        store.search_filtered(vector, top_k, filter).await
    }

    /// The configured vector store, when present.
    pub fn vector_store(&self) -> Option<Arc<dyn VectorStore>> {
        self.vector_store.clone()
    }

    /// Readiness flag served by `GET /ready` (liveness at `/health` is
    /// always `ok`). Starts `true`; flip it while warming caches or draining
    /// before shutdown.
    pub fn set_ready(&self, ready: bool) {
        self.ready
            .store(ready, std::sync::atomic::Ordering::Relaxed);
    }

    /// Current readiness state.
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Relaxed)
    }
}
