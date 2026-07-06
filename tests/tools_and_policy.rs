//! Integration tests for the tool-calling loop, usage accounting, and the
//! skill sandbox — all offline via a scripted mock LLM.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use corrosive_agents::llm::Role;
use corrosive_agents::prelude::*;
use futures_util::stream::BoxStream;
use serde_json::json;

/// A scripted LLM: first turn requests the `add` tool, second turn answers
/// with the tool's result. Reports fixed token usage on every call.
struct ToolScriptLlm {
    calls: AtomicUsize,
}

impl ToolScriptLlm {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for ToolScriptLlm {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let call_number = self.calls.fetch_add(1, Ordering::SeqCst);
        let usage = Some(corrosive_agents::llm::Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        });
        if call_number == 0 {
            assert!(
                request.tools.as_ref().is_some_and(|t| !t.is_empty()),
                "tools should be offered to the model"
            );
            Ok(ChatResponse {
                content: String::new(),
                model: "scripted".into(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "add".into(),
                    arguments: json!({ "a": 2, "b": 40 }),
                }],
                usage,
            })
        } else {
            // The tool result must have been fed back.
            let tool_message = request
                .messages
                .iter()
                .find(|m| matches!(m.role, Role::Tool))
                .expect("tool result message present");
            assert_eq!(tool_message.tool_call_id.as_deref(), Some("call-1"));
            assert!(tool_message.content.contains("42"));
            Ok(ChatResponse {
                content: "the answer is 42".into(),
                model: "scripted".into(),
                tool_calls: Vec::new(),
                usage,
            })
        }
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>> {
        unimplemented!("not used in these tests")
    }

    fn default_model(&self) -> &str {
        "scripted"
    }
}

fn add_skill() -> FnSkill {
    FnSkill::new("add", "Adds a and b", |input| async move {
        let sum = input["a"].as_i64().unwrap_or(0) + input["b"].as_i64().unwrap_or(0);
        Ok(json!({ "sum": sum }))
    })
}

#[tokio::test]
async fn tool_loop_executes_skills_and_records_history() {
    let agent = Agent::builder()
        .name("tools")
        .version("1.0.0")
        .llm(ToolScriptLlm::new())
        .skill(add_skill())
        .build()
        .unwrap();

    let reply = agent
        .chat_with_tools("s", "what is 2 + 40?", 5)
        .await
        .unwrap();
    assert_eq!(reply, "the answer is 42");

    // Session: user, assistant(tool_calls), tool result, final assistant.
    let history = agent.session_history("s").await.unwrap();
    assert_eq!(history.len(), 4);
    assert!(history[1].tool_calls.is_some());
    assert!(matches!(history[2].role, Role::Tool));
    assert_eq!(history[3].content, "the answer is 42");
}

#[tokio::test]
async fn usage_totals_and_observer_fire() {
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_clone = Arc::clone(&seen);

    let agent = Agent::builder()
        .name("metered")
        .version("1.0.0")
        .llm(ToolScriptLlm::new())
        .skill(add_skill())
        .usage_observer(move |event: &UsageEvent| {
            assert_eq!(event.model, "scripted");
            seen_clone.fetch_add(1, Ordering::SeqCst);
        })
        .build()
        .unwrap();

    agent.chat_with_tools("s", "sum please", 5).await.unwrap();

    // Two completions happened (tool round + final answer).
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    let totals = agent.usage();
    assert_eq!(totals.requests, 2);
    assert_eq!(totals.total_tokens, 30);
    assert_eq!(totals.prompt_tokens, 20);
}

#[tokio::test]
async fn allowlist_blocks_unlisted_skills() {
    let agent = Agent::builder()
        .name("locked")
        .version("1.0.0")
        .skill(add_skill())
        .skill(FnSkill::new("noop", "does nothing", |_| async move {
            Ok(json!({}))
        }))
        .skill_policy(SkillPolicy::new().allow_only(["noop"]))
        .build()
        .unwrap();

    agent.execute_skill("noop", json!({})).await.unwrap();
    let err = agent.execute_skill("add", json!({})).await.unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)), "got: {err}");
}

#[tokio::test]
async fn required_permissions_are_enforced() {
    let net_skill = || {
        FnSkill::new("fetch", "Fetches a URL", |_| async move { Ok(json!({})) })
            .with_permissions(["net"])
    };

    // Not granted → refused.
    let denied = Agent::builder()
        .name("sandboxed")
        .version("1.0.0")
        .skill(net_skill())
        .build()
        .unwrap();
    let err = denied.execute_skill("fetch", json!({})).await.unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)));

    // Granted → runs.
    let granted = Agent::builder()
        .name("sandboxed")
        .version("1.0.0")
        .skill(net_skill())
        .skill_policy(SkillPolicy::new().grant("net"))
        .build()
        .unwrap();
    granted.execute_skill("fetch", json!({})).await.unwrap();
}

#[tokio::test]
async fn skill_timeout_fires() {
    let agent = Agent::builder()
        .name("slowpoke")
        .version("1.0.0")
        .skill(FnSkill::new("sleepy", "sleeps forever", |_| async move {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(json!({}))
        }))
        .skill_policy(SkillPolicy::new().with_timeout(Duration::from_millis(50)))
        .build()
        .unwrap();

    let err = agent.execute_skill("sleepy", json!({})).await.unwrap_err();
    assert!(
        matches!(&err, Error::Skill(msg) if msg.contains("timed out")),
        "got: {err}"
    );
}

#[tokio::test]
async fn panicking_skill_does_not_take_down_the_agent() {
    let agent = Agent::builder()
        .name("resilient")
        .version("1.0.0")
        .skill(FnSkill::new("boom", "panics", |_| async move {
            panic!("kaboom");
        }))
        .skill(add_skill())
        .build()
        .unwrap();

    let err = agent.execute_skill("boom", json!({})).await.unwrap_err();
    assert!(
        matches!(&err, Error::Skill(msg) if msg.contains("panicked")),
        "got: {err}"
    );

    // The agent still works afterwards.
    let out = agent
        .execute_skill("add", json!({ "a": 1, "b": 1 }))
        .await
        .unwrap();
    assert_eq!(out["sum"], 2);
}

#[tokio::test]
async fn tool_loop_gives_up_after_max_rounds() {
    /// Always demands another tool call — never converges.
    struct LoopingLlm;

    #[async_trait::async_trait]
    impl LlmProvider for LoopingLlm {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse> {
            Ok(ChatResponse {
                content: String::new(),
                model: "looper".into(),
                tool_calls: vec![ToolCall {
                    id: "again".into(),
                    name: "add".into(),
                    arguments: json!({ "a": 1, "b": 1 }),
                }],
                usage: None,
            })
        }
        async fn chat_stream(
            &self,
            _request: ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamChunk>>> {
            unimplemented!()
        }
        fn default_model(&self) -> &str {
            "looper"
        }
    }

    let agent = Agent::builder()
        .name("bounded")
        .version("1.0.0")
        .llm(LoopingLlm)
        .skill(add_skill())
        .build()
        .unwrap();

    let err = agent.chat_with_tools("s", "loop", 3).await.unwrap_err();
    assert!(
        matches!(&err, Error::Llm(msg) if msg.contains("3 rounds")),
        "got: {err}"
    );
}
