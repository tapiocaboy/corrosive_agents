//! Vector stores for retrieval-augmented agents.
//!
//! One trait — [`VectorStore`] — with three ways to satisfy it:
//!
//! - [`InMemoryVectorStore`]: zero-dependency reference implementation
//!   (cosine similarity), also a template for **custom** stores.
//! - [`QdrantStore`] (feature `qdrant`): Qdrant via its REST API.
//! - [`PineconeStore`] (feature `pinecone`): Pinecone via its REST API.
//!
//! Implement the trait yourself to plug in any other backend.

mod memory;
#[cfg(feature = "pinecone")]
mod pinecone;
#[cfg(feature = "qdrant")]
mod qdrant;

pub use memory::InMemoryVectorStore;
#[cfg(feature = "pinecone")]
#[cfg_attr(docsrs, doc(cfg(feature = "pinecone")))]
pub use pinecone::PineconeStore;
#[cfg(feature = "qdrant")]
#[cfg_attr(docsrs, doc(cfg(feature = "qdrant")))]
pub use qdrant::QdrantStore;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// A document stored in (or destined for) a vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Stable identifier.
    pub id: String,
    /// The embedding vector.
    pub vector: Vec<f32>,
    /// Original text, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Arbitrary JSON metadata.
    #[serde(default)]
    pub metadata: Value,
}

impl Document {
    /// Create a document from an id and vector.
    pub fn new(id: impl Into<String>, vector: Vec<f32>) -> Self {
        Self {
            id: id.into(),
            vector,
            text: None,
            metadata: Value::Null,
        }
    }

    /// Attach the original text.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Attach JSON metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// One hit from a similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Id of the matched document.
    pub id: String,
    /// Similarity score (higher is more similar).
    pub score: f32,
    /// Original text, when stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Stored metadata.
    #[serde(default)]
    pub metadata: Value,
}

/// A pluggable vector database.
///
/// Implement this trait to bring your own store; see
/// [`InMemoryVectorStore`] for a complete reference implementation.
#[async_trait::async_trait]
pub trait VectorStore: Send + Sync {
    /// Insert or overwrite documents.
    async fn upsert(&self, documents: Vec<Document>) -> Result<()>;

    /// Return the `top_k` most similar documents to `vector`.
    async fn search(&self, vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>>;

    /// Delete documents by id.
    async fn delete(&self, ids: &[String]) -> Result<()>;
}
