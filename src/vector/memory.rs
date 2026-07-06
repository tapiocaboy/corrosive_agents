//! In-memory vector store — reference implementation and custom-store template.

use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::error::{Error, Result};
use crate::vector::{Document, MetadataFilter, SearchResult, VectorStore};

/// A thread-safe in-memory vector store using cosine similarity.
///
/// Useful for tests, examples, and small corpora — and as the template to
/// copy when implementing [`VectorStore`] for a custom backend.
#[derive(Debug, Default)]
pub struct InMemoryVectorStore {
    documents: RwLock<HashMap<String, Document>>,
}

impl InMemoryVectorStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored documents.
    pub async fn len(&self) -> usize {
        self.documents.read().await.len()
    }

    /// `true` when the store holds no documents.
    pub async fn is_empty(&self) -> bool {
        self.documents.read().await.is_empty()
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::MIN;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return f32::MIN;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

#[async_trait::async_trait]
impl VectorStore for InMemoryVectorStore {
    async fn upsert(&self, documents: Vec<Document>) -> Result<()> {
        if let Some(bad) = documents.iter().find(|d| d.vector.is_empty()) {
            return Err(Error::VectorStore(format!(
                "document '{}' has an empty vector",
                bad.id
            )));
        }
        let mut store = self.documents.write().await;
        for document in documents {
            store.insert(document.id.clone(), document);
        }
        Ok(())
    }

    async fn search(&self, vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>> {
        self.search_filtered(vector, top_k, &MetadataFilter::new())
            .await
    }

    async fn search_filtered(
        &self,
        vector: Vec<f32>,
        top_k: usize,
        filter: &MetadataFilter,
    ) -> Result<Vec<SearchResult>> {
        let store = self.documents.read().await;
        let mut results: Vec<SearchResult> = store
            .values()
            .filter(|doc| filter.is_empty() || filter.matches(&doc.metadata))
            .map(|doc| SearchResult {
                id: doc.id.clone(),
                score: cosine_similarity(&vector, &doc.vector),
                text: doc.text.clone(),
                metadata: doc.metadata.clone(),
            })
            .collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);
        Ok(results)
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        let mut store = self.documents.write().await;
        for id in ids {
            store.remove(id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn search_ranks_by_similarity() {
        let store = InMemoryVectorStore::new();
        store
            .upsert(vec![
                Document::new("x", vec![1.0, 0.0]).with_text("east"),
                Document::new("y", vec![0.0, 1.0]).with_text("north"),
                Document::new("xy", vec![0.7, 0.7]).with_text("northeast"),
            ])
            .await
            .unwrap();

        let results = store.search(vec![1.0, 0.1], 2).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "x");
        assert_eq!(results[1].id, "xy");
    }

    #[tokio::test]
    async fn upsert_overwrites_and_delete_removes() {
        let store = InMemoryVectorStore::new();
        store
            .upsert(vec![
                Document::new("a", vec![1.0]).with_metadata(json!({"v": 1}))
            ])
            .await
            .unwrap();
        store
            .upsert(vec![
                Document::new("a", vec![1.0]).with_metadata(json!({"v": 2}))
            ])
            .await
            .unwrap();
        assert_eq!(store.len().await, 1);

        let results = store.search(vec![1.0], 5).await.unwrap();
        assert_eq!(results[0].metadata["v"], 2);

        store.delete(&["a".to_string()]).await.unwrap();
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn empty_vector_rejected() {
        let store = InMemoryVectorStore::new();
        assert!(store
            .upsert(vec![Document::new("bad", vec![])])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn metadata_filter_narrows_results() {
        let store = InMemoryVectorStore::new();
        store
            .upsert(vec![
                Document::new("r1", vec![1.0, 0.0]).with_metadata(json!({"lang": "rust"})),
                Document::new("g1", vec![1.0, 0.0]).with_metadata(json!({"lang": "go"})),
                Document::new("r2", vec![0.9, 0.1]).with_metadata(json!({"lang": "rust"})),
            ])
            .await
            .unwrap();

        let filter = MetadataFilter::new().eq("lang", json!("rust"));
        let hits = store
            .search_filtered(vec![1.0, 0.0], 10, &filter)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.metadata["lang"] == "rust"));
    }

    #[tokio::test]
    async fn batched_upsert_stores_everything() {
        let store = InMemoryVectorStore::new();
        let docs: Vec<Document> = (0..25)
            .map(|i| Document::new(format!("d{i}"), vec![i as f32, 1.0]))
            .collect();
        store.upsert_batched(docs, 10).await.unwrap();
        assert_eq!(store.len().await, 25);
    }
}
