//! Pinecone vector store backend (REST API), enabled with feature `pinecone`.

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::vector::{Document, SearchResult, VectorStore};

/// A [`VectorStore`] backed by a [Pinecone](https://www.pinecone.io) index.
///
/// Construct with the **index host** shown in the Pinecone console
/// (e.g. `https://my-index-abc123.svc.aped-4627-b74a.pinecone.io`).
///
/// Note: Pinecone metadata must be flat (strings, numbers, booleans, lists of
/// strings); nested metadata is stored JSON-encoded under `_metadata`.
#[derive(Debug, Clone)]
pub struct PineconeStore {
    http: reqwest::Client,
    host: String,
    api_key: String,
    namespace: String,
}

impl PineconeStore {
    /// Create a store for the index at `host` using `api_key`.
    pub fn new(host: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            host: host.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            namespace: String::new(),
        }
    }

    /// Scope operations to a namespace.
    #[must_use]
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}{path}", self.host))
            .header("Api-Key", &self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(Error::VectorStore(format!(
                "Pinecone returned {status}: {body}"
            )));
        }
        Ok(body)
    }
}

fn to_pinecone_metadata(doc: &Document) -> Value {
    let mut metadata = Map::new();
    if let Some(text) = &doc.text {
        metadata.insert("_text".into(), Value::String(text.clone()));
    }
    match &doc.metadata {
        Value::Null => {}
        Value::Object(map) => {
            for (key, value) in map {
                match value {
                    Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                        metadata.insert(key.clone(), value.clone());
                    }
                    other => {
                        metadata.insert(key.clone(), Value::String(other.to_string()));
                    }
                }
            }
        }
        other => {
            metadata.insert("_metadata".into(), Value::String(other.to_string()));
        }
    }
    Value::Object(metadata)
}

#[async_trait::async_trait]
impl VectorStore for PineconeStore {
    async fn upsert(&self, documents: Vec<Document>) -> Result<()> {
        let vectors: Vec<Value> = documents
            .iter()
            .map(|doc| {
                json!({
                    "id": doc.id,
                    "values": doc.vector,
                    "metadata": to_pinecone_metadata(doc),
                })
            })
            .collect();
        self.post(
            "/vectors/upsert",
            json!({ "vectors": vectors, "namespace": self.namespace }),
        )
        .await?;
        Ok(())
    }

    async fn search(&self, vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>> {
        let body = json!({
            "vector": vector,
            "topK": top_k,
            "includeMetadata": true,
            "namespace": self.namespace,
        });
        let response = self.post("/query", body).await?;
        let matches = response
            .get("matches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(matches
            .into_iter()
            .map(|hit| {
                let mut metadata = hit.get("metadata").cloned().unwrap_or(Value::Null);
                let text = metadata
                    .get("_text")
                    .and_then(Value::as_str)
                    .map(String::from);
                if let Value::Object(map) = &mut metadata {
                    map.remove("_text");
                }
                SearchResult {
                    id: hit
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    score: hit.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                    text,
                    metadata,
                }
            })
            .collect())
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        self.post(
            "/vectors/delete",
            json!({ "ids": ids, "namespace": self.namespace }),
        )
        .await?;
        Ok(())
    }
}
