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

mod chunk;
mod memory;
#[cfg(feature = "pgvector")]
mod pg;
#[cfg(feature = "pinecone")]
mod pinecone;
#[cfg(feature = "qdrant")]
mod qdrant;

pub use chunk::chunk_text;
pub use memory::InMemoryVectorStore;
#[cfg(feature = "pgvector")]
#[cfg_attr(docsrs, doc(cfg(feature = "pgvector")))]
pub use pg::PgVectorStore;
#[cfg(feature = "pinecone")]
#[cfg_attr(docsrs, doc(cfg(feature = "pinecone")))]
pub use pinecone::PineconeStore;
#[cfg(feature = "qdrant")]
#[cfg_attr(docsrs, doc(cfg(feature = "qdrant")))]
pub use qdrant::QdrantStore;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// An equality filter over document metadata.
///
/// Backends translate it natively where possible (Qdrant payload filters,
/// Pinecone `$eq` filters, pgvector `@>` containment); the trait's default
/// implementation over-fetches and filters client-side, so custom stores get
/// filtering for free.
///
/// ```
/// use corrosive_agents::vector::MetadataFilter;
/// use serde_json::json;
///
/// let filter = MetadataFilter::new()
///     .eq("topic", json!("rust"))
///     .eq("year", json!(2026));
/// assert!(filter.matches(&json!({ "topic": "rust", "year": 2026, "extra": 1 })));
/// assert!(!filter.matches(&json!({ "topic": "go" })));
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MetadataFilter {
    /// Field → required value (all must match).
    pub equals: BTreeMap<String, Value>,
}

impl MetadataFilter {
    /// An empty filter (matches everything).
    pub fn new() -> Self {
        Self::default()
    }

    /// Require `metadata[key] == value`.
    #[must_use]
    pub fn eq(mut self, key: impl Into<String>, value: Value) -> Self {
        self.equals.insert(key.into(), value);
        self
    }

    /// `true` when no conditions are set.
    pub fn is_empty(&self) -> bool {
        self.equals.is_empty()
    }

    /// Does `metadata` satisfy every condition?
    pub fn matches(&self, metadata: &Value) -> bool {
        self.equals
            .iter()
            .all(|(key, expected)| metadata.get(key) == Some(expected))
    }
}

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

    /// Like [`search`](Self::search), keeping only documents whose metadata
    /// matches `filter`.
    ///
    /// The default implementation over-fetches (4×) and filters client-side;
    /// backends override it with native filters where the database supports
    /// them.
    async fn search_filtered(
        &self,
        vector: Vec<f32>,
        top_k: usize,
        filter: &MetadataFilter,
    ) -> Result<Vec<SearchResult>> {
        if filter.is_empty() {
            return self.search(vector, top_k).await;
        }
        let fetch = top_k.saturating_mul(4).max(top_k);
        let results = self.search(vector, fetch).await?;
        Ok(results
            .into_iter()
            .filter(|r| filter.matches(&r.metadata))
            .take(top_k)
            .collect())
    }

    /// Upsert in fixed-size batches — use for large corpora to keep request
    /// sizes bounded. `batch_size` of 0 is treated as 1.
    async fn upsert_batched(&self, documents: Vec<Document>, batch_size: usize) -> Result<()> {
        let batch_size = batch_size.max(1);
        // Consume the Vec in chunks without cloning documents.
        let mut documents = documents;
        while !documents.is_empty() {
            let rest = documents.split_off(documents.len().min(batch_size));
            self.upsert(documents).await?;
            documents = rest;
        }
        Ok(())
    }

    /// Delete documents by id.
    async fn delete(&self, ids: &[String]) -> Result<()>;
}
