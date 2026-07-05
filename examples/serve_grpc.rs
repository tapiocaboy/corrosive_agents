//! Serve an agent over gRPC on port 50051.
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...
//! cargo run --example serve_grpc --features grpc
//! ```
//!
//! Try it with grpcurl (reflection is not enabled, so pass the proto):
//!
//! ```sh
//! grpcurl -plaintext -proto proto/agent.proto \
//!   -d '{}' 127.0.0.1:50051 corrosive.agent.v1.AgentService/GetInfo
//! grpcurl -plaintext -proto proto/agent.proto \
//!   -d '{"message":"Hello!"}' 127.0.0.1:50051 corrosive.agent.v1.AgentService/Chat
//! grpcurl -plaintext -proto proto/agent.proto \
//!   -d '{"name":"reverse","input_json":"{\"text\":\"corrosive\"}"}' \
//!   127.0.0.1:50051 corrosive.agent.v1.AgentService/ExecuteSkill
//! ```
//!
//! Or with the generated Rust client:
//! `corrosive_agents::grpc::pb::agent_service_client::AgentServiceClient`.

use std::sync::Arc;

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt().init();

    let agent = Agent::builder()
        .name("grpc-agent")
        .version("0.1.0")
        .description("An agent served over gRPC")
        .system_prompt("You are a helpful assistant.")
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

    println!("serving '{}' on grpc://127.0.0.1:50051", agent.name());
    Arc::new(agent)
        .serve_grpc("127.0.0.1:50051".parse().unwrap())
        .await
}
