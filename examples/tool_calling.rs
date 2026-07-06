//! Tool calling: the model automatically invokes the agent's skills.
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...
//! cargo run --example tool_calling
//! ```

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let agent = Agent::builder()
        .name("calculator-agent")
        .version("0.1.0")
        .system_prompt(
            "You are a precise assistant. Use the provided tools for any arithmetic; \
             never compute it yourself.",
        )
        .model(models::LLAMA_NEMOTRON_SUPER_49B)
        .skill(
            FnSkill::new(
                "multiply",
                "Multiplies two integers a and b",
                |input| async move {
                    let a = input["a"].as_i64().unwrap_or(0);
                    let b = input["b"].as_i64().unwrap_or(0);
                    println!("  [skill] multiply({a}, {b})");
                    Ok(json!({ "product": a * b }))
                },
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "a": { "type": "integer" },
                    "b": { "type": "integer" }
                },
                "required": ["a", "b"]
            })),
        )
        .usage_observer(|event: &UsageEvent| {
            println!(
                "  [usage] {} tokens ({})",
                event.usage.total_tokens, event.model
            );
        })
        .llm(NvidiaClient::from_env()?)
        .build()?;

    let question = "What is 1337 multiplied by 42? Use the tool.";
    println!("user > {question}");
    let reply = agent.chat_with_tools("demo", question, 5).await?;
    println!("agent> {reply}");

    println!("\ntotal usage: {:?}", agent.usage());
    Ok(())
}
