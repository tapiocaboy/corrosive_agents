//! Integration tests for A2A delegation, persistent sessions, trust chains,
//! and DID identities — all offline (mock LLM, ephemeral local servers).

use std::sync::Arc;

use corrosive_agents::prelude::*;
use futures_util::stream::BoxStream;
use serde_json::json;

/// A deterministic LLM that answers with `echo:<last user message>`.
struct EchoLlm;

#[async_trait::async_trait]
impl LlmProvider for EchoLlm {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, corrosive_agents::llm::Role::User))
            .map(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ChatResponse {
            content: format!("echo:{last_user}"),
            model: "echo".into(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>> {
        let reply = self.chat(request).await?.content;
        let chunks = vec![
            Ok(StreamChunk {
                delta: reply,
                done: false,
            }),
            Ok(StreamChunk {
                delta: String::new(),
                done: true,
            }),
        ];
        Ok(Box::pin(futures_util::stream::iter(chunks)))
    }

    fn default_model(&self) -> &str {
        "echo"
    }
}

#[tokio::test]
async fn chat_history_flows_through_the_session_store() {
    let agent = Agent::builder()
        .name("hist")
        .version("1.0.0")
        .llm(EchoLlm)
        .build()
        .unwrap();

    agent.chat("s1", "one").await.unwrap();
    agent.chat("s1", "two").await.unwrap();

    let history = agent.session_history("s1").await.unwrap();
    assert_eq!(history.len(), 4); // user, assistant, user, assistant
    assert_eq!(history[3].content, "echo:two");
    assert_eq!(agent.list_sessions().await.unwrap(), vec!["s1"]);

    agent.clear_session("s1").await.unwrap();
    assert!(agent.session_history("s1").await.unwrap().is_empty());
}

#[cfg(feature = "sqlite-sessions")]
#[tokio::test]
async fn sessions_survive_agent_restart_with_sqlite() {
    let dir = std::env::temp_dir().join(format!("corrosive-a2a-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("sessions.db");

    {
        let agent = Agent::builder()
            .name("persistent")
            .version("1.0.0")
            .llm(EchoLlm)
            .session_store(SqliteSessionStore::open(&db).unwrap())
            .build()
            .unwrap();
        agent.chat("job", "remember me").await.unwrap();
    }

    // A brand-new agent process (simulated) sees the same history.
    let reborn = Agent::builder()
        .name("persistent")
        .version("1.0.1")
        .llm(EchoLlm)
        .session_store(SqliteSessionStore::open(&db).unwrap())
        .build()
        .unwrap();
    let history = reborn.session_history("job").await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "remember me");

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "server")]
mod a2a {
    use super::*;

    /// Serve `agent` on an ephemeral port; returns its base URL.
    async fn spawn_peer(agent: Agent) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = corrosive_agents::server::router(Arc::new(agent));
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn worker_agent() -> (Agent, AgentIdentity) {
        let identity = AgentIdentity::generate();
        let agent = Agent::builder()
            .name("worker")
            .version("1.0.0")
            .llm(EchoLlm)
            .skill(FnSkill::new("double", "Doubles n", |input| async move {
                Ok(json!({ "n": input["n"].as_i64().unwrap_or(0) * 2 }))
            }))
            .identity(identity.clone())
            .build()
            .unwrap();
        (agent, identity)
    }

    #[tokio::test]
    async fn delegate_chat_and_skill_to_verified_peer() {
        let (worker, worker_identity) = worker_agent();
        let url = spawn_peer(worker).await;

        let orchestrator = Agent::builder()
            .name("orchestrator")
            .version("1.0.0")
            .peer(
                "worker",
                // Pin by DID to exercise the did:key path end-to-end.
                RemoteAgent::new(&url).with_pinned_key(worker_identity.did_key()),
            )
            .build()
            .unwrap();

        let reply = orchestrator
            .delegate_chat("worker", "job-1", "hello")
            .await
            .unwrap();
        assert_eq!(reply, "echo:hello");

        let out = orchestrator
            .delegate_skill("worker", "double", json!({ "n": 21 }))
            .await
            .unwrap();
        assert_eq!(out["n"], 42);

        assert_eq!(orchestrator.list_peers().await, vec!["worker"]);
    }

    #[tokio::test]
    async fn delegation_refused_when_pinned_key_mismatches() {
        let (worker, _) = worker_agent();
        let url = spawn_peer(worker).await;

        let imposter_key = AgentIdentity::generate().public_key_base64();
        let orchestrator = Agent::builder()
            .name("orchestrator")
            .version("1.0.0")
            .peer(
                "worker",
                RemoteAgent::new(&url).with_pinned_key(imposter_key),
            )
            .build()
            .unwrap();

        let err = orchestrator
            .delegate_chat("worker", "job-1", "hello")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::A2a(_)), "got: {err}");
    }

    #[tokio::test]
    async fn unregistered_peer_is_an_error() {
        let solo = Agent::builder()
            .name("solo")
            .version("1.0.0")
            .build()
            .unwrap();
        let err = solo.delegate_chat("ghost", "s", "hi").await.unwrap_err();
        assert!(matches!(err, Error::A2a(_)));
    }

    #[tokio::test]
    async fn remote_verify_returns_the_manifest() {
        let (worker, identity) = worker_agent();
        let url = spawn_peer(worker).await;

        let remote = RemoteAgent::new(&url).with_pinned_key(identity.public_key_base64());
        let manifest = remote.verify().await.unwrap();
        assert_eq!(manifest.name, "worker");
        assert_eq!(manifest.did, Some(identity.did_key()));

        let info = remote.info().await.unwrap();
        assert!(info.skills.contains(&"double".to_string()));
    }
}
