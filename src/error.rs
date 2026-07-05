//! Unified error and result types for the crate.

/// All the ways a corrosive agent can fail.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Invalid or missing configuration (builder validation, env vars, …).
    #[error("configuration error: {0}")]
    Config(String),

    /// JSON (de)serialization failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP transport failure (NVIDIA NIM, Pinecone, Qdrant, …).
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// The LLM provider returned an error or an unusable response.
    #[error("LLM provider error: {0}")]
    Llm(String),

    /// Key material could not be parsed or used.
    #[error("identity error: {0}")]
    Identity(String),

    /// A cryptographic signature did not verify.
    #[error("verification failed: {0}")]
    Verification(String),

    /// A vector store operation failed.
    #[error("vector store error: {0}")]
    VectorStore(String),

    /// An MCP server misbehaved or the JSON-RPC exchange failed.
    #[error("MCP error: {0}")]
    Mcp(String),

    /// No skill registered under the requested name.
    #[error("skill '{0}' not found")]
    SkillNotFound(String),

    /// A skill ran but failed.
    #[error("skill execution error: {0}")]
    Skill(String),

    /// Transport-layer serving failure (REST/WS/gRPC).
    #[error("server error: {0}")]
    Server(String),

    /// Agent-to-agent delegation failure (peer unreachable, untrusted, or
    /// returned an error).
    #[error("A2A error: {0}")]
    A2a(String),

    /// The agent was asked to do something it was not built for
    /// (e.g. `chat` without an LLM provider).
    #[error("agent has no {0} configured")]
    NotConfigured(&'static str),
}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
