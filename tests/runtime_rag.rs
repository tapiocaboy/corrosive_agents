//! Agent runtime coverage: chat, streaming, RAG (remember/recall), info, and
//! the "not configured" error paths — all offline via fake providers.

mod common;

use common::{EchoLlm, HashEmbed};
use corrosive_agents::prelude::*;
use futures_util::StreamExt;
use serde_json::json;

fn rag_agent() -> Agent {
    Agent::builder()
        .name("rag")
        .version("1.0.0")
        .description("retrieval agent")
        .system_prompt("be helpful")
        .capability(Capability::new("chat", "conversation"))
        .llm(EchoLlm::default())
        .embeddings(HashEmbed::default())
        .vector_store(InMemoryVectorStore::new())
        .skill(FnSkill::new("echo", "echoes", |i| async move { Ok(i) }))
        .generate_identity()
        .build()
        .unwrap()
}

#[tokio::test]
async fn chat_records_history_and_usage() {
    let agent = rag_agent();
    let reply = agent.chat("s1", "hello").await.unwrap();
    assert_eq!(reply, "echo: hello");

    let history = agent.session_history("s1").await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "hello");
    assert_eq!(history[1].content, "echo: hello");

    let usage = agent.usage();
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.total_tokens, 5);

    // Session management.
    assert!(agent
        .list_sessions()
        .await
        .unwrap()
        .contains(&"s1".to_string()));
    agent.clear_session("s1").await.unwrap();
    assert!(agent.session_history("s1").await.unwrap().is_empty());
}

#[tokio::test]
async fn chat_stream_yields_chunks_and_persists() {
    let agent = rag_agent();
    let mut stream = agent.chat_stream("s2", "hi").await.unwrap();

    let mut text = String::new();
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        if chunk.done {
            saw_done = true;
        } else {
            text.push_str(&chunk.delta);
        }
    }
    assert_eq!(text, "hello");
    assert!(saw_done);

    // The full reply was appended to the session on stream completion.
    let history = agent.session_history("s2").await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content, "hello");
}

#[tokio::test]
async fn remember_and_recall_round_trip() {
    let agent = rag_agent();

    let id = agent
        .remember("apple pie recipe", json!({ "kind": "food" }))
        .await
        .unwrap();
    assert!(!id.is_empty());
    agent
        .remember("rust borrow checker", json!({ "kind": "code" }))
        .await
        .unwrap();

    let hits = agent.recall("apple pie recipe", 3).await.unwrap();
    assert!(!hits.is_empty());
    assert!(hits[0].text.as_deref().unwrap().contains("apple pie"));

    // Filtered recall restricts by metadata.
    let mut filter = MetadataFilter::new();
    filter.equals.insert("kind".into(), json!("code"));
    let filtered = agent.recall_filtered("rust", 5, &filter).await.unwrap();
    assert!(filtered
        .iter()
        .all(|r| r.text.as_deref() != Some("apple pie recipe")));

    assert!(agent.vector_store().is_some());
}

#[tokio::test]
async fn remember_batch_and_document() {
    let agent = rag_agent();

    let ids = agent
        .remember_batch(&["one".into(), "two".into()], json!({}))
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);

    // Empty batch is a no-op.
    assert!(agent
        .remember_batch(&[], json!({}))
        .await
        .unwrap()
        .is_empty());

    let long = "word ".repeat(200);
    let doc_ids = agent
        .remember_document(&long, json!({ "src": "doc" }), 100, 10)
        .await
        .unwrap();
    assert!(
        doc_ids.len() > 1,
        "a long document should produce multiple chunks"
    );
}

#[tokio::test]
async fn info_and_capabilities_reflect_the_build() {
    let agent = rag_agent();
    let info = agent.info();
    assert_eq!(info.name, "rag");
    assert_eq!(info.version, "1.0.0");
    assert!(info.skills.contains(&"echo".to_string()));
    assert!(info.signed, "generate_identity signs the manifest");
    assert!(info.public_key.is_some());

    let active = agent.active_capabilities();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].name, "chat");

    assert!(agent.is_ready());
    agent.set_ready(false);
    assert!(!agent.is_ready());
}

#[tokio::test]
async fn missing_providers_report_not_configured() {
    // No LLM configured → chat fails with NotConfigured.
    let no_llm = Agent::builder()
        .name("bare")
        .version("1.0.0")
        .build()
        .unwrap();
    let err = no_llm.chat("s", "hi").await.unwrap_err();
    assert!(
        matches!(err, Error::NotConfigured("LLM provider")),
        "got: {err}"
    );

    // No embeddings/vector store → remember/recall fail with NotConfigured.
    let no_rag = Agent::builder()
        .name("bare2")
        .version("1.0.0")
        .llm(EchoLlm::default())
        .build()
        .unwrap();
    let err = no_rag.remember("x", json!({})).await.unwrap_err();
    assert!(matches!(err, Error::NotConfigured(_)), "got: {err}");
    let err = no_rag.recall("x", 1).await.unwrap_err();
    assert!(matches!(err, Error::NotConfigured(_)), "got: {err}");
}
