//! LLM integration: provider traits, chat types, and the NVIDIA NIM client.

mod nvidia;
mod types;
mod usage;

pub use nvidia::{models, NvidiaClient, RetryPolicy};
pub use types::{
    ChatMessage, ChatRequest, ChatResponse, Role, StreamChunk, ToolCall, ToolSpec, Usage,
};
pub use usage::{UsageEvent, UsageObserver, UsageSnapshot, UsageTotals};

use futures_util::stream::BoxStream;

use crate::error::Result;

/// A chat-completion backend. Implement this to plug any LLM into an agent;
/// [`NvidiaClient`] is the built-in implementation for NVIDIA NIM / Nemotron.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Run a chat completion and return the full response.
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// Run a streaming chat completion, yielding incremental deltas.
    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>>;

    /// The default model id used when a request does not specify one.
    fn default_model(&self) -> &str;
}

/// A text-embedding backend used for retrieval (RAG) against a
/// [`VectorStore`](crate::vector::VectorStore).
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed documents for indexing.
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed a search query. Kept separate because retrieval models such as
    /// `nv-embedqa-e5-v5` embed queries and passages differently.
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}
