# corrosive_agents

[![CI](https://github.com/tapiocaboy/corrosive_agents/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/tapiocaboy/corrosive_agents/actions/workflows/ci.yml)
[![Security audit](https://github.com/tapiocaboy/corrosive_agents/actions/workflows/audit.yml/badge.svg)](https://github.com/tapiocaboy/corrosive_agents/actions/workflows/audit.yml)
[![codecov](https://codecov.io/gh/tapiocaboy/corrosive_agents/branch/main/graph/badge.svg)](https://codecov.io/gh/tapiocaboy/corrosive_agents)
[![Crates.io](https://img.shields.io/crates/v/corrosive_agents.svg)](https://crates.io/crates/corrosive_agents)
[![docs.rs](https://img.shields.io/docsrs/corrosive_agents)](https://docs.rs/corrosive_agents)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)

Build **verifiable, interactive AI agents** in Rust, powered by
[NVIDIA Nemotron](https://build.nvidia.com) free LLM models.

A *corrosive agent* has a name, a version, and a set of active capabilities —
all loadable from a JSON manifest — plus native **Skills**, **MCP** (Model
Context Protocol) servers, and an **Ed25519 identity** so anyone can verify
it with public-key cryptography. Serve it over a Tokio **REST API**,
**WebSocket**, or **gRPC**, and give it memory with **Pinecone**, **Qdrant**,
or any **custom vector store**.

## Features

| | |
|---|---|
| 🏗️ **Builder pattern** | Fluent `Agent::builder()…build()` construction with semver + config validation |
| 📄 **JSON manifests** | Load name/version/capabilities/skills/MCP servers from a file |
| 🔐 **Verifiable identity** | Ed25519 manifests; `did:key` DIDs; X.509 certs; key rotation & revocation (`TrustStore`) |
| 🧠 **NVIDIA Nemotron** | Chat, streaming, tool calling, embeddings — with retry/backoff + rate-limit handling |
| 🛠️ **Skills** | Async JSON abilities with a sandbox: allowlists, permissions, timeouts, panic isolation |
| 🔁 **Tool loop** | `chat_with_tools`: the model auto-invokes skills; usage accounting hooks built in |
| 🔌 **MCP** | stdio + streamable-HTTP/SSE transports; tools, resources, and prompts |
| 🌐 **Transports** | REST + WebSocket + gRPC, with API-key/JWT auth, TLS, graceful shutdown, `/ready`, OpenAPI |
| 🤝 **A2A delegation** | `RemoteAgent` peers with pinned-key/DID verification; delegate chat & skills |
| 💾 **Sessions** | Pluggable `SessionStore`: in-memory, SQLite, or Redis persistence |
| 📚 **Vector stores** | In-memory, Qdrant, Pinecone, pgvector; metadata filters, chunking, `remember`/`recall` |

📖 **New to the library? Read the [tutorial](docs/TUTORIAL.md)** — it walks
from an empty project to a production-shaped agent, and ships on
[docs.rs](https://docs.rs/corrosive_agents) as the `tutorial` module.

## Installation

```toml
[dependencies]
corrosive_agents = "0.0.1"          # REST + WebSocket by default
tokio = { version = "1", features = ["full"] }
```

Optional features:

```toml
corrosive_agents = { version = "0.0.1", features = ["full"] } # everything
```

| Feature           | Default | Enables                                        |
|-------------------|---------|------------------------------------------------|
| `server`          | ✅      | REST + WebSocket serving + auth middleware      |
| `grpc`            | —       | gRPC serving + generated client (tonic)         |
| `tls`             | —       | TLS helpers for REST and gRPC                   |
| `openapi`         | —       | OpenAPI 3 document at `/openapi.json`           |
| `x509`            | —       | X.509 certificate-based identity                |
| `pinecone`        | —       | Pinecone vector store backend                   |
| `qdrant`          | —       | Qdrant vector store backend                     |
| `pgvector`        | —       | PostgreSQL/pgvector vector store backend        |
| `sqlite-sessions` | —       | SQLite-persisted conversation history           |
| `redis-sessions`  | —       | Redis-persisted conversation history            |
| `full`            | —       | All of the above                                |

## Quickstart

Get a free API key at [build.nvidia.com](https://build.nvidia.com) and export
it as `NVIDIA_API_KEY`.

```rust,no_run
use corrosive_agents::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = Agent::builder()
        .name("research-agent")
        .version("0.1.0")
        .description("A concise research assistant")
        .system_prompt("Answer in at most three sentences.")
        .model(models::NEMOTRON_3_NANO_30B)
        .capability(Capability::new("chat", "Conversational Q&A"))
        .llm(NvidiaClient::from_env()?)
        .generate_identity()          // manifest is signed at build()
        .build()?;

    let reply = agent.chat("session-1", "What is NVIDIA Nemotron?").await?;
    println!("{reply}");
    Ok(())
}
```

### Load an agent from JSON

```json
{
  "name": "manifest-agent",
  "version": "1.0.0",
  "model": "nvidia/llama-3.3-nemotron-super-49b-v1",
  "capabilities": [{ "name": "chat", "description": "Conversational Q&A" }],
  "skills": ["word_count"],
  "mcp_servers": [
    { "name": "fs", "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] }
  ]
}
```

```rust,no_run
use corrosive_agents::prelude::*;
# fn skill_impl() -> FnSkill { FnSkill::new("word_count", "", |i| async move { Ok(i) }) }

# async fn run() -> Result<()> {
let agent = AgentBuilder::from_json_file("agent.json")?
    .skill(skill_impl())              // implementations bind to declared names
    .llm(NvidiaClient::from_env()?)
    .generate_identity()
    .build()?;

agent.connect_mcp_servers().await?;   // spawn + handshake declared MCP servers
let tools = agent.mcp_tools("fs").await?;
# Ok(())
# }
```

### Verify an agent with public-key cryptography

```rust
use corrosive_agents::prelude::*;

# fn main() -> Result<()> {
let agent = Agent::builder()
    .name("trusted").version("1.0.0")
    .generate_identity()
    .build()?;

// Ship the manifest anywhere as JSON…
let json = agent.manifest().to_json()?;

// …and anyone can verify it, offline:
let received = AgentManifest::from_json(&json)?;
received.verify()?;                            // embedded public key
received.verify_with(&agent.public_key().unwrap())?; // or a pinned key
# Ok(())
# }
```

Tampering with any signed field makes verification fail.

### Serve it

```rust,no_run
use std::sync::Arc;
use corrosive_agents::prelude::*;

# async fn run() -> Result<()> {
# let agent = Agent::builder().name("a").version("1.0.0").build()?;
let agent = Arc::new(agent);

// REST + WebSocket (feature `server`, on by default)
agent.clone().serve("0.0.0.0:8080".parse().unwrap()).await?;

// gRPC (feature `grpc`)
// agent.serve_grpc("0.0.0.0:50051".parse().unwrap()).await?;
# Ok(())
# }
```

REST endpoints: `GET /health`, `GET /agent`, `GET /agent/manifest`,
`GET /capabilities`, `GET /skills`, `POST /skills/{name}`, `POST /chat`,
`POST /verify`, and a WebSocket at `/ws` with optional streamed chunks.

gRPC service (`proto/agent.proto`): `GetInfo`, `Chat`, `ChatStream`
(server streaming), `ExecuteSkill` — a generated Rust client ships with the
crate at `corrosive_agents::grpc::pb::agent_service_client::AgentServiceClient`.

### RAG with a vector store

```rust,no_run
use corrosive_agents::prelude::*;
use serde_json::json;

# async fn run() -> Result<()> {
let nvidia = NvidiaClient::from_env()?;
let agent = Agent::builder()
    .name("rag").version("0.1.0")
    .llm(nvidia.clone())
    .embeddings(nvidia)                       // NvidiaClient embeds too
    .vector_store(InMemoryVectorStore::new()) // or QdrantStore / PineconeStore
    .build()?;

agent.remember("Nemotron models are free at build.nvidia.com", json!({"topic": "nvidia"})).await?;
let hits = agent.recall("where are Nemotron models hosted?", 3).await?;
# Ok(())
# }
```

Bring your own store by implementing the `VectorStore` trait (three async
methods: `upsert`, `search`, `delete`).

## Examples

| Example | Shows | Run |
|---|---|---|
| `build_agent` | Builder pattern, capabilities, skills, signed manifest, chat | `cargo run --example build_agent` |
| `agent_from_json` | Loading an agent + MCP config from `examples/agent.json` | `cargo run --example agent_from_json` |
| `interactive_chat` | Terminal REPL with streamed tokens | `cargo run --example interactive_chat` |
| `tool_calling` | Model auto-invokes skills (function calling) + usage hooks | `cargo run --example tool_calling` |
| `sign_and_verify` | Public-key verification end to end (offline) | `cargo run --example sign_and_verify` |
| `vector_rag` | Embeddings + vector store + retrieval-augmented answers | `cargo run --example vector_rag` |
| `serve` | REST + WebSocket server | `cargo run --example serve` |
| `serve_grpc` | gRPC server | `cargo run --example serve_grpc --features grpc` |
| `a2a_delegation` | Agent-to-agent delegation with DID-pinned peer verification (offline) | `cargo run --example a2a_delegation` |

All LLM examples need `NVIDIA_API_KEY` (or `NVIDIA_KEY` in a `.env` file).

## Nemotron models

Constants in `corrosive_agents::llm::models`, all usable with a free key:

- `nvidia/nemotron-3-ultra-550b-a55b`, `nvidia/nemotron-3-super-120b-a12b`,
  `nvidia/nemotron-3-nano-30b-a3b` (default)
- `nvidia/llama-3.1-nemotron-ultra-253b-v1`, `nvidia/llama-3.3-nemotron-super-49b-v1`,
  `nvidia/llama-3.1-nemotron-nano-8b-v1`, `nvidia/nemotron-mini-4b-instruct`
- Embeddings: `nvidia/nv-embedqa-e5-v5`, `nvidia/llama-nemotron-embed-1b-v2`

The catalog evolves — list what your key can reach with
`GET https://integrate.api.nvidia.com/v1/models`.

## Development

```sh
cargo test --features full          # unit + integration + doc tests
cargo clippy --all-targets --features full
cargo fmt --check
cargo deny check                    # advisories, licenses, bans, sources (deny.toml)
cargo llvm-cov --features full      # code coverage report
cargo run --manifest-path tools/protogen/Cargo.toml   # regen gRPC code after editing proto/
```


## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
