//! Retrieval-augmented generation: embed facts with NVIDIA embeddings, store
//! them in a vector store, and answer questions with retrieved context.
//!
//! Uses the in-memory store by default. Swap in Qdrant or Pinecone by
//! enabling the feature and changing one builder line (see comments).
//!
//! ```sh
//! export NVIDIA_API_KEY=nvapi-...
//! cargo run --example vector_rag
//! ```

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let nvidia = NvidiaClient::from_env()?;

    let agent = Agent::builder()
        .name("rag-agent")
        .version("0.1.0")
        .capability(Capability::new("rag", "Retrieval-augmented answers"))
        .llm(nvidia.clone())
        .embeddings(nvidia) // NvidiaClient is also an EmbeddingProvider
        .vector_store(InMemoryVectorStore::new())
        // Qdrant instead (needs `--features qdrant` and a running Qdrant):
        //   .vector_store(QdrantStore::new("http://localhost:6333", "facts"))
        // Pinecone instead (needs `--features pinecone`):
        //   .vector_store(PineconeStore::new("https://<index-host>", api_key))
        .build()?;

    // Index a tiny knowledge base.
    for (fact, topic) in [
        (
            "The crate corrosive_agents builds verifiable AI agents in Rust.",
            "crate",
        ),
        (
            "Nemotron models are served for free at build.nvidia.com with an API key.",
            "nvidia",
        ),
        (
            "Agents sign their manifests with Ed25519 keys so anyone can verify them.",
            "crypto",
        ),
        (
            "Qdrant and Pinecone are supported vector stores; custom stores implement one trait.",
            "vector",
        ),
    ] {
        let id = agent.remember(fact, json!({ "topic": topic })).await?;
        println!("remembered [{topic}] as {id}");
    }

    // Retrieve, then answer with the context stitched into the prompt.
    let question = "How do I verify that an agent is authentic?";
    let hits = agent.recall(question, 2).await?;
    println!("\ntop matches:");
    for hit in &hits {
        println!(
            "  {:.3}  {}",
            hit.score,
            hit.text.as_deref().unwrap_or("<no text>")
        );
    }

    let context: Vec<String> = hits.iter().filter_map(|h| h.text.clone()).collect();
    let prompt = format!(
        "Answer using only this context:\n{}\n\nQuestion: {question}",
        context.join("\n")
    );
    let answer = agent.chat("rag-demo", prompt).await?;
    println!("\nassistant: {answer}");

    Ok(())
}
