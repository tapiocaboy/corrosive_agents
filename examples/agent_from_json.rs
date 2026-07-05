//! Load an agent from a JSON manifest (examples/agent.json), register the
//! skill implementations it declares, and chat.
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...
//! cargo run --example agent_from_json
//! ```

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    // Everything declarative — name, version, capabilities, model, system
    // prompt, skill names, MCP servers — comes from the JSON file.
    let agent = AgentBuilder::from_json_file("examples/agent.json")?
        // Skill *implementations* are code, registered against the declared name.
        .skill(FnSkill::new(
            "word_count",
            "Counts words in `text`",
            |input| async move {
                let words = input["text"]
                    .as_str()
                    .unwrap_or_default()
                    .split_whitespace()
                    .count();
                Ok(json!({ "words": words }))
            },
        ))
        .llm(NvidiaClient::from_env()?)
        .generate_identity()
        .build()?;

    println!(
        "loaded '{}' v{} — {}",
        agent.name(),
        agent.version(),
        agent.manifest().description
    );
    for capability in agent.active_capabilities() {
        println!(
            "  capability: {} ({})",
            capability.name, capability.description
        );
    }

    let counted = agent
        .execute_skill(
            "word_count",
            json!({ "text": "corrosive agents eat rust for breakfast" }),
        )
        .await?;
    println!("word_count -> {counted}");

    let reply = agent.chat("json-demo", "What can you do?").await?;
    println!("\nassistant: {reply}");

    // Optional: connect the MCP servers declared in the manifest.
    // Requires npx + @modelcontextprotocol/server-filesystem to be available:
    //
    //   let connected = agent.connect_mcp_servers().await?;
    //   let tools = agent.mcp_tools("fs").await?;
    //   let listing = agent.call_mcp_tool("fs", "list_directory", json!({"path": "/tmp"})).await?;

    Ok(())
}
