//! Integration tests exercising the public API — no network required.

use std::sync::Arc;

use corrosive_agents::prelude::*;
use serde_json::json;

fn sample_manifest_json() -> String {
    r#"{
        "name": "it-agent",
        "version": "2.0.0",
        "description": "integration test agent",
        "capabilities": [
            { "name": "chat", "description": "talks" },
            { "name": "batch", "description": "off", "enabled": false }
        ],
        "skills": ["echo"],
        "mcp_servers": [
            { "name": "fs", "command": "npx", "args": ["-y", "some-server"] }
        ]
    }"#
    .to_string()
}

#[tokio::test]
async fn agent_from_json_manifest_end_to_end() {
    let agent = AgentBuilder::from_json(&sample_manifest_json())
        .unwrap()
        .skill(FnSkill::new("echo", "Echoes input", |input| async move {
            Ok(input)
        }))
        .generate_identity()
        .build()
        .unwrap();

    // Manifest fields survived the JSON load.
    assert_eq!(agent.name(), "it-agent");
    assert_eq!(agent.version(), "2.0.0");
    assert_eq!(agent.manifest().mcp_servers[0].name, "fs");

    // Only enabled capabilities are active.
    let active: Vec<_> = agent
        .active_capabilities()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(active, vec!["chat"]);

    // Identity signed the manifest; verification passes and is portable.
    agent.verify().unwrap();
    let exported = agent.manifest().to_json().unwrap();
    AgentManifest::from_json(&exported)
        .unwrap()
        .verify()
        .unwrap();

    // Skills execute.
    let out = agent
        .execute_skill("echo", json!({ "ping": true }))
        .await
        .unwrap();
    assert_eq!(out["ping"], true);

    // Info snapshot is consistent.
    let info = agent.info();
    assert!(info.signed);
    assert_eq!(info.skills, vec!["echo"]);
    assert_eq!(info.public_key, agent.public_key());
}

#[tokio::test]
async fn custom_vector_store_via_trait_object() {
    // The in-memory store *is* the custom-store path: anything implementing
    // VectorStore plugs into the builder the same way.
    let agent = Agent::builder()
        .name("vec-agent")
        .version("0.1.0")
        .vector_store(InMemoryVectorStore::new())
        .build()
        .unwrap();

    let store = agent.vector_store().expect("store configured");
    store
        .upsert(vec![
            Document::new("a", vec![1.0, 0.0]).with_text("alpha"),
            Document::new("b", vec![0.0, 1.0]).with_text("beta"),
        ])
        .await
        .unwrap();

    let hits = store.search(vec![0.9, 0.1], 1).await.unwrap();
    assert_eq!(hits[0].id, "a");
    assert_eq!(hits[0].text.as_deref(), Some("alpha"));
}

#[tokio::test]
async fn remember_requires_configuration() {
    let agent = Agent::builder()
        .name("bare")
        .version("0.1.0")
        .build()
        .unwrap();
    let err = agent
        .remember("fact", serde_json::Value::Null)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotConfigured(_)));
}

#[tokio::test]
async fn agent_is_shareable_across_tasks() {
    let agent = Arc::new(
        Agent::builder()
            .name("shared")
            .version("0.1.0")
            .skill(FnSkill::new("noop", "does nothing", |_| async move {
                Ok(json!({}))
            }))
            .build()
            .unwrap(),
    );

    let mut handles = Vec::new();
    for _ in 0..8 {
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent.execute_skill("noop", json!({})).await.unwrap()
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
}

#[test]
fn tampered_manifest_fails_verification() {
    let identity = AgentIdentity::generate();
    let mut manifest = AgentManifest::new("victim", "1.0.0");
    manifest.sign(&identity).unwrap();

    let mut tampered = manifest.clone();
    tampered.name = "attacker".into();
    assert!(tampered.verify().is_err());
    assert!(manifest.verify().is_ok());
}
