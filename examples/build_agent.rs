//! Build an agent with the Builder pattern and run a single chat turn.
//!
//! Requires a free NVIDIA API key from <https://build.nvidia.com>:
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...   # or put NVIDIA_KEY=... in .env
//! cargo run --example build_agent
//! ```

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv(); // pick up NVIDIA_KEY from .env if present

    let agent = Agent::builder()
        .name("research-agent")
        .version("0.1.0")
        .description("A concise research assistant")
        .system_prompt("You are a concise research assistant. Answer in at most three sentences.")
        .model(models::NEMOTRON_3_NANO_30B)
        .capability(Capability::new("chat", "Conversational Q&A"))
        .capability(
            Capability::new("summarize", "Summarizes text")
                .with_config(json!({ "max_words": 100 })),
        )
        .skill(FnSkill::new(
            "shout",
            "Uppercases the input text",
            |input| async move {
                let text = input["text"].as_str().unwrap_or_default();
                Ok(json!({ "text": text.to_uppercase() }))
            },
        ))
        .llm(NvidiaClient::from_env()?)
        .generate_identity() // manifest is signed automatically at build()
        .build()?;

    println!("agent      : {} v{}", agent.name(), agent.version());
    println!("public key : {}", agent.public_key().unwrap_or_default());
    println!("verified   : {:?}", agent.verify().is_ok());

    // Skills run locally, no LLM involved.
    let shouted = agent
        .execute_skill("shout", json!({ "text": "hello nemotron" }))
        .await?;
    println!("skill out  : {shouted}");

    // Chat turns share history within a session id.
    let reply = agent
        .chat("demo", "In one sentence, what is NVIDIA Nemotron?")
        .await?;
    println!("\nassistant  : {reply}");

    let follow_up = agent.chat("demo", "Name one model in that family.").await?;
    println!("assistant  : {follow_up}");

    Ok(())
}
