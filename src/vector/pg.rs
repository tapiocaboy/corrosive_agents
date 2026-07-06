//! pgvector (PostgreSQL) vector store backend, enabled with feature
//! `pgvector`.

use std::sync::Arc;

use pgvector::Vector;
use serde_json::Value;
use tokio_postgres::NoTls;

use crate::error::{Error, Result};
use crate::vector::{Document, MetadataFilter, SearchResult, VectorStore};

fn pg_err(e: tokio_postgres::Error) -> Error {
    Error::VectorStore(format!("pgvector: {e}"))
}

/// A [`VectorStore`] backed by PostgreSQL with the
/// [pgvector](https://github.com/pgvector/pgvector) extension.
///
/// Documents live in a table `(id TEXT PRIMARY KEY, embedding vector(n),
/// text TEXT, metadata JSONB)`; similarity is cosine (`<=>` operator) and
/// metadata filters use JSONB containment (`@>`), so they can be served by a
/// GIN index.
///
/// ```no_run
/// use corrosive_agents::vector::PgVectorStore;
///
/// # async fn run() -> corrosive_agents::Result<()> {
/// let store = PgVectorStore::connect(
///     "host=localhost user=postgres password=secret dbname=agents",
///     "documents",
/// )
/// .await?;
/// store.ensure_table(1024).await?; // installs the extension + table if missing
/// # Ok(())
/// # }
/// ```
///
/// Connections are unencrypted (`NoTls`); front Postgres with TLS at the
/// network layer or a proxy if needed.
#[derive(Clone)]
pub struct PgVectorStore {
    client: Arc<tokio_postgres::Client>,
    table: String,
}

impl std::fmt::Debug for PgVectorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgVectorStore")
            .field("table", &self.table)
            .finish_non_exhaustive()
    }
}

fn validate_table_name(table: &str) -> Result<()> {
    let valid = !table.is_empty() && table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "invalid table name '{table}': use only ASCII letters, digits, and underscores"
        )))
    }
}

impl PgVectorStore {
    /// Connect with a `tokio_postgres` connection string and target table.
    ///
    /// The connection driver is spawned onto the current Tokio runtime.
    pub async fn connect(connection_string: &str, table: impl Into<String>) -> Result<Self> {
        let table = table.into();
        validate_table_name(&table)?;
        let (client, connection) = tokio_postgres::connect(connection_string, NoTls)
            .await
            .map_err(pg_err)?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("pgvector connection error: {e}");
            }
        });
        Ok(Self {
            client: Arc::new(client),
            table,
        })
    }

    /// Install the `vector` extension and create the table + cosine index if
    /// they do not exist. `dimensions` must match your embedding model
    /// (e.g. 1024 for `nv-embedqa-e5-v5`).
    pub async fn ensure_table(&self, dimensions: usize) -> Result<()> {
        let table = &self.table;
        let ddl = format!(
            "CREATE EXTENSION IF NOT EXISTS vector;
             CREATE TABLE IF NOT EXISTS {table} (
                 id        TEXT PRIMARY KEY,
                 embedding vector({dimensions}) NOT NULL,
                 text      TEXT,
                 metadata  JSONB NOT NULL DEFAULT 'null'::jsonb
             );
             CREATE INDEX IF NOT EXISTS {table}_metadata_idx ON {table} USING GIN (metadata);"
        );
        self.client.batch_execute(&ddl).await.map_err(pg_err)
    }

    async fn run_query(
        &self,
        vector: Vec<f32>,
        top_k: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        let embedding = Vector::from(vector);
        let limit = top_k as i64;
        let table = &self.table;

        let rows = if let Some(filter) = filter {
            let conditions = Value::Object(
                filter
                    .equals
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            let sql = format!(
                "SELECT id, text, metadata, 1 - (embedding <=> $1) AS score
                 FROM {table} WHERE metadata @> $3 ORDER BY embedding <=> $1 LIMIT $2"
            );
            self.client
                .query(&sql, &[&embedding, &limit, &conditions])
                .await
                .map_err(pg_err)?
        } else {
            let sql = format!(
                "SELECT id, text, metadata, 1 - (embedding <=> $1) AS score
                 FROM {table} ORDER BY embedding <=> $1 LIMIT $2"
            );
            self.client
                .query(&sql, &[&embedding, &limit])
                .await
                .map_err(pg_err)?
        };

        Ok(rows
            .into_iter()
            .map(|row| SearchResult {
                id: row.get::<_, String>(0),
                text: row.get::<_, Option<String>>(1),
                metadata: row.get::<_, Value>(2),
                score: row.get::<_, f64>(3) as f32,
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl VectorStore for PgVectorStore {
    async fn upsert(&self, documents: Vec<Document>) -> Result<()> {
        let table = &self.table;
        let sql = format!(
            "INSERT INTO {table} (id, embedding, text, metadata) VALUES ($1, $2, $3, $4)
             ON CONFLICT (id) DO UPDATE
             SET embedding = EXCLUDED.embedding,
                 text = EXCLUDED.text,
                 metadata = EXCLUDED.metadata"
        );
        let statement = self.client.prepare(&sql).await.map_err(pg_err)?;
        for doc in documents {
            let embedding = Vector::from(doc.vector);
            self.client
                .execute(&statement, &[&doc.id, &embedding, &doc.text, &doc.metadata])
                .await
                .map_err(pg_err)?;
        }
        Ok(())
    }

    async fn search(&self, vector: Vec<f32>, top_k: usize) -> Result<Vec<SearchResult>> {
        self.run_query(vector, top_k, None).await
    }

    async fn search_filtered(
        &self,
        vector: Vec<f32>,
        top_k: usize,
        filter: &MetadataFilter,
    ) -> Result<Vec<SearchResult>> {
        if filter.is_empty() {
            return self.run_query(vector, top_k, None).await;
        }
        self.run_query(vector, top_k, Some(filter)).await
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        let table = &self.table;
        let sql = format!("DELETE FROM {table} WHERE id = ANY($1)");
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.client.execute(&sql, &[&ids]).await.map_err(pg_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_names_are_validated() {
        assert!(validate_table_name("documents").is_ok());
        assert!(validate_table_name("docs_v2").is_ok());
        assert!(validate_table_name("docs; DROP TABLE users").is_err());
        assert!(validate_table_name("").is_err());
    }

    /// Full roundtrip against a real Postgres with pgvector. Skipped unless
    /// `PG_URL` is set (e.g. `PG_URL="host=localhost user=postgres" cargo
    /// test --features pgvector`).
    #[tokio::test]
    async fn roundtrip_against_real_postgres() {
        let Ok(url) = std::env::var("PG_URL") else {
            eprintln!("PG_URL not set — skipping pgvector integration test");
            return;
        };
        let table = format!("corrosive_test_{}", uuid::Uuid::new_v4().simple());
        let store = PgVectorStore::connect(&url, &table).await.unwrap();
        store.ensure_table(2).await.unwrap();

        store
            .upsert(vec![
                Document::new("a", vec![1.0, 0.0])
                    .with_text("alpha")
                    .with_metadata(json!({"lang": "rust"})),
                Document::new("b", vec![0.0, 1.0]).with_metadata(json!({"lang": "go"})),
            ])
            .await
            .unwrap();

        let hits = store.search(vec![1.0, 0.1], 1).await.unwrap();
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[0].text.as_deref(), Some("alpha"));

        let filter = MetadataFilter::new().eq("lang", json!("go"));
        let hits = store
            .search_filtered(vec![1.0, 0.0], 5, &filter)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "b");

        store.delete(&["a".into(), "b".into()]).await.unwrap();
        assert!(store.search(vec![1.0, 0.0], 5).await.unwrap().is_empty());

        store
            .client
            .batch_execute(&format!("DROP TABLE {table}"))
            .await
            .unwrap();
    }
}
