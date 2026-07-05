//! Interactive terminal REPL with streamed responses.
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...
//! cargo run --example interactive_chat
//! ```

use std::io::Write as _;

use corrosive_agents::prelude::*;
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let agent = Agent::builder()
        .name("repl-agent")
        .version("0.1.0")
        .system_prompt("You are a friendly, concise assistant.")
        .llm(NvidiaClient::from_env()?.with_model(models::NEMOTRON_3_NANO_30B))
        .build()?;

    println!(
        "Interactive chat with {} (model: {})",
        agent.name(),
        models::NEMOTRON_3_NANO_30B
    );
    println!("Type a message and press Enter. Ctrl-D or 'exit' to quit.\n");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("you> ");
        std::io::stdout().flush()?;
        let Some(line) = lines.next_line().await? else {
            break;
        };
        let message = line.trim();
        if message.is_empty() {
            continue;
        }
        if message.eq_ignore_ascii_case("exit") {
            break;
        }

        print!("agent> ");
        std::io::stdout().flush()?;
        let mut stream = agent.chat_stream("repl", message).await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if chunk.done {
                break;
            }
            print!("{}", chunk.delta);
            std::io::stdout().flush()?;
        }
        println!("\n");
    }

    println!("bye!");
    Ok(())
}
