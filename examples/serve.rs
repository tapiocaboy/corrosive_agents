//! Serve an agent over REST + WebSocket on port 8080.
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...
//! cargo run --example serve
//! ```
//!
//! Then, from another terminal:
//!
//! ```sh
//! curl http://127.0.0.1:8080/health
//! curl http://127.0.0.1:8080/agent
//! curl http://127.0.0.1:8080/agent/manifest
//! curl -X POST http://127.0.0.1:8080/skills/reverse \
//!      -H 'content-type: application/json' -d '{"text":"corrosive"}'
//! curl -X POST http://127.0.0.1:8080/chat \
//!      -H 'content-type: application/json' -d '{"message":"Hello!"}'
//! # WebSocket (e.g. with websocat):
//! #   websocat ws://127.0.0.1:8080/ws
//! #   {"type":"chat","message":"Hi there","stream":true}
//! ```

use std::sync::Arc;

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt().init();

    let agent = Agent::builder()
        .name("http-agent")
        .version("0.1.0")
        .description("An agent served over REST and WebSocket")
        .system_prompt("You are a helpful assistant.")
        .capability(Capability::new("chat", "Conversational Q&A"))
        .skill(FnSkill::new(
            "reverse",
            "Reverses the input text",
            |input| async move {
                let text: String = input["text"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .rev()
                    .collect();
                Ok(json!({ "text": text }))
            },
        ))
        .llm(NvidiaClient::from_env()?)
        .generate_identity()
        .build()?;

    println!(
        "serving '{}' on http://127.0.0.1:8080 (WS on /ws)",
        agent.name()
    );
    Arc::new(agent)
        .serve("127.0.0.1:8080".parse().unwrap())
        .await
}
