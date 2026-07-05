//! Qdrant vector store backend (REST API), enabled with feature `qdrant`.

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::vector::{Document, SearchResult, VectorStore};

/// UUID v5 namespace for deterministically mapping document ids to Qdrant
/// point ids (Qdrant only accepts unsigned integers or UUIDs as point ids).
const ID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0xc0, 0x44, 0x05, 0x1e, 0xa9, 0x3e, 0x4c, 0x11, 0x9d, 0x1a, 0x6e, 0x2b, 0x7a, 0x11, 0x22, 0x33,
]);

fn point_id(document_id: &str) -> String {
    uuid::Uuid::new_v5(&ID_NAMESPACE, document_id.as_bytes()).to_string()
}

/// A [`VectorStore`] backed by a [Qdrant](https://qdrant.tech) collection.
///
/// ```no_run
/// use corrosive_agents::vector::QdrantStore;
///
/// # async fn run() -> corrosive_agents::Result<()> {
/// let store = QdrantStore::new("http://localhost:6334", "my-collection")
///     .with_api_key("optional-api-key");
/// store.ensure_collection(1024).await?; // create if missing (cosine distance)
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct QdrantStore {
    http: reqwest::Client,
    base_url: String,
    collection: String,
    api_key: Option<String>,
}

impl QdrantStore {
    /// Create a store for `collection` at `base_url`
    /// (e.g. `http://localhost:6333` for local REST).
    pub fn new(base_url: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            collection: collection.into(),
            api_key: None,
        }
    }

    /// Set the `api-key` header (Qdrant Cloud).
    #[must_use]
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Create the collection with cosine distance if it does not exist.
    pub async fn ensure_collection(&self, vector_size: usize) -> Result<()> {
        let url = format!("{}/collections/{}", self.base_url, self.collection);
        let exists = self.send(self.http.get(&url)).await;
        if exists.is_ok() {
            return Ok(());
        }
        let body = json!({ "vectors": { "size": vector_size, "distance": "Cosine" } });
        self.send(self.http.put(&url).json(&body)).await?;
        Ok(())
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value> {
        let request = match &self.api_key {
            Some(key) => request.header("api-key", key),
            None => request,
        };
        let response = request.send().await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(Error::VectorStore(format!(
                "Qdrant returned {status}: {body}"
            )));
        }
        Ok(body)
    }
}

#[async_trait::async_trait]
impl VectorStore for QdrantStore {
    async fn upsert(&self, documents: Vec<Document>) -> Result<()> {
        let points: Vec<Value> = documents
            .into_iter()
            .map(|doc| {
                json!({
                    "id": point_id(&doc.id),
                    "vector": doc.vector,
                    "payload": {
                        "_id": doc.id,
                        "_text": doc.text,
                        "metadata": doc.metadata,
                    },
                })
            })
            .collect();
        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.base_url, self.collection
        );
        self.send(self.http.put(&url).json(&json!({ "points": points })))
            .await?;
        Ok(())
    }

    async fn search(&self, vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url, self.collection
        );
        let body = json!({ "vector": vector, "limit": top_k, "with_payload": true });
        let response = self.send(self.http.post(&url).json(&body)).await?;
        let hits = response
            .get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(hits
            .into_iter()
            .map(|hit| {
                let payload = hit.get("payload").cloned().unwrap_or(Value::Null);
                SearchResult {
                    id: payload
                        .get("_id")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| {
                            hit.get("id").map(|v| v.to_string()).unwrap_or_default()
                        }),
                    score: hit.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                    text: payload
                        .get("_text")
                        .and_then(Value::as_str)
                        .map(String::from),
                    metadata: payload.get("metadata").cloned().unwrap_or(Value::Null),
                }
            })
            .collect())
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        let points: Vec<String> = ids.iter().map(|id| point_id(id)).collect();
        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.base_url, self.collection
        );
        self.send(self.http.post(&url).json(&json!({ "points": points })))
            .await?;
        Ok(())
    }
}
