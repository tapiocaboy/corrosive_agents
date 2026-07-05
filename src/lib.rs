#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # corrosive_agents
//!
//! Build **verifiable, interactive AI agents** powered by
//! [NVIDIA Nemotron](https://build.nvidia.com) free LLM models.
//!
//! ## Highlights
//!
//! - **Builder pattern** — assemble an [`Agent`] fluently with [`AgentBuilder`].
//! - **JSON manifests** — an agent's name, version, capabilities, skills and
//!   MCP servers load from a JSON file ([`AgentManifest`]).
//! - **Verifiable identity** — Ed25519 signatures over the manifest
//!   ([`identity::AgentIdentity`]); anyone can verify an agent with its
//!   public key.
//! - **NVIDIA NIM client** — chat + streaming + embeddings against the
//!   OpenAI-compatible `integrate.api.nvidia.com` endpoint
//!   ([`llm::NvidiaClient`]).
//! - **Skills & MCP** — native async [`skills::Skill`]s plus a
//!   [Model Context Protocol](https://modelcontextprotocol.io) stdio client.
//! - **Multi-transport** — Tokio REST API + WebSocket (feature `server`),
//!   and gRPC (feature `grpc`).
//! - **Vector stores** — one [`vector::VectorStore`] trait; Pinecone
//!   (feature `pinecone`), Qdrant (feature `qdrant`), an in-memory reference
//!   implementation, or your own custom store.
//!
//! ## Quickstart
//!
//! ```no_run
//! use corrosive_agents::prelude::*;
//!
//! # async fn run() -> corrosive_agents::Result<()> {
//! let agent = Agent::builder()
//!     .name("research-agent")
//!     .version("0.1.0")
//!     .description("Answers research questions")
//!     .capability(Capability::new("chat", "Conversational Q&A"))
//!     .llm(NvidiaClient::from_env()?.with_model(models::LLAMA_NEMOTRON_SUPER_49B))
//!     .generate_identity()
//!     .build()?;
//!
//! let reply = agent.chat("session-1", "What is Rust?").await?;
//! println!("{reply}");
//! # Ok(())
//! # }
//! ```
//!
//! ## Feature flags
//!
//! | Feature    | Default | Enables                                   |
//! |------------|---------|-------------------------------------------|
//! | `server`   | yes     | REST + WebSocket serving (axum)           |
//! | `grpc`     | no      | gRPC serving (tonic, vendored protos)     |
//! | `pinecone` | no      | Pinecone vector store backend             |
//! | `qdrant`   | no      | Qdrant vector store backend               |
//! | `full`     | no      | All of the above                          |

pub mod agent;
pub mod error;
pub mod identity;
pub mod llm;
pub mod mcp;
pub mod skills;
pub mod vector;

#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;

#[cfg(feature = "grpc")]
#[cfg_attr(docsrs, doc(cfg(feature = "grpc")))]
pub mod grpc;

pub use agent::{Agent, AgentBuilder, AgentInfo, AgentManifest, Capability};
pub use error::{Error, Result};

/// Convenient glob-import for the most commonly used types.
///
/// ```
/// use corrosive_agents::prelude::*;
/// ```
pub mod prelude {
    pub use crate::agent::{Agent, AgentBuilder, AgentInfo, AgentManifest, Capability};
    pub use crate::error::{Error, Result};
    pub use crate::identity::AgentIdentity;
    pub use crate::llm::{
        models, ChatMessage, ChatRequest, ChatResponse, EmbeddingProvider, LlmProvider,
        NvidiaClient, StreamChunk,
    };
    pub use crate::mcp::{McpClient, McpServerConfig, McpTool};
    pub use crate::skills::{FnSkill, Skill, SkillRegistry};
    pub use crate::vector::{Document, InMemoryVectorStore, SearchResult, VectorStore};

    #[cfg(feature = "pinecone")]
    pub use crate::vector::PineconeStore;
    #[cfg(feature = "qdrant")]
    pub use crate::vector::QdrantStore;
}
