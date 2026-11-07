//! Vector-store HTTP backends (features `pinecone` + `qdrant`): upsert, search,
//! filtered search, delete, and error handling — against a mock REST server.
#![cfg(all(feature = "pinecone", feature = "qdrant"))]

mod common;

use common::mock_http;
use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::test]
async fn pinecone_upsert_search_delete() {
    let url = mock_http(|_m, path, _b| {
        if path.ends_with("/query") {
            (
                200,
                r#"{"matches":[{"id":"d1","score":0.88,"metadata":{"_text":"hello world","kind":"greeting"}}]}"#
                    .to_string(),
            )
        } else if path.ends_with("/vectors/upsert") || path.ends_with("/vectors/delete") {
            (200, "{}".to_string())
        } else {
            (404, "{}".to_string())
        }
    })
    .await;

    let store = PineconeStore::new(url, "api-key").with_namespace("ns");
    store
        .upsert(vec![Document::new("d1", vec![0.1, 0.2])
            .with_text("hello world")
            .with_metadata(json!({ "kind": "greeting" }))])
        .await
        .unwrap();

    let hits = store.search(vec![0.1, 0.2], 3).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d1");
    assert_eq!(hits[0].text.as_deref(), Some("hello world"));
    assert!((hits[0].score - 0.88).abs() < 1e-6);
    // The `_text` field is lifted out of the metadata.
    assert!(hits[0].metadata.get("_text").is_none());
    assert_eq!(hits[0].metadata.get("kind").unwrap(), "greeting");

    let mut filter = MetadataFilter::new();
    filter.equals.insert("kind".into(), json!("greeting"));
    let hits = store.search_filtered(vec![0.1, 0.2], 3, &filter).await.unwrap();
    assert_eq!(hits.len(), 1);

    store.delete(&["d1".to_string()]).await.unwrap();
}

#[tokio::test]
async fn pinecone_surfaces_errors() {
    let url = mock_http(|_m, _p, _b| (500, r#"{"message":"boom"}"#.to_string())).await;
    let store = PineconeStore::new(url, "key");
    let err = store.search(vec![0.0], 1).await.unwrap_err();
    assert!(matches!(err, Error::VectorStore(_)), "got: {err}");
}

#[tokio::test]
async fn qdrant_ensure_upsert_search_delete() {
    let url = mock_http(|method, path, _b| {
        if path.contains("/points/search") {
            (
                200,
                r#"{"result":[{"id":"u","score":0.9,"payload":{"_id":"d1","_text":"hi","metadata":{"k":"v"}}}]}"#
                    .to_string(),
            )
        } else if path.contains("/points") {
            (200, "{}".to_string()) // upsert or delete
        } else if path.contains("/collections/") {
            // ensure_collection: GET → not found (triggers create), PUT → ok.
            if method == "GET" {
                (404, "{}".to_string())
            } else {
                (200, "{}".to_string())
            }
        } else {
            (404, "{}".to_string())
        }
    })
    .await;

    let store = QdrantStore::new(url, "testcol").with_api_key("key");
    store.ensure_collection(2).await.unwrap();
    store
        .upsert(vec![Document::new("d1", vec![0.1, 0.2])
            .with_text("hi")
            .with_metadata(json!({ "k": "v" }))])
        .await
        .unwrap();

    let hits = store.search(vec![0.1, 0.2], 5).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d1");
    assert_eq!(hits[0].text.as_deref(), Some("hi"));

    let mut filter = MetadataFilter::new();
    filter.equals.insert("k".into(), json!("v"));
    let hits = store.search_filtered(vec![0.1, 0.2], 5, &filter).await.unwrap();
    assert_eq!(hits.len(), 1);

    store.delete(&["d1".to_string()]).await.unwrap();
}

#[tokio::test]
async fn qdrant_ensure_collection_early_returns_when_present() {
    let url = mock_http(|_m, path, _b| {
        if path.contains("/collections/") {
            (200, "{}".to_string())
        } else {
            (404, "{}".to_string())
        }
    })
    .await;
    // GET returns 200 → collection exists → no create call.
    QdrantStore::new(url, "col").ensure_collection(2).await.unwrap();
}
