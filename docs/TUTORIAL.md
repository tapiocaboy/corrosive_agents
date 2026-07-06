# Building agents with `corrosive_agents` — the tutorial

This walks you from an empty project to a production-shaped agent:
chat → skills → tool calling → memory (RAG) → identity & trust → serving
(REST/WebSocket/gRPC with auth and TLS) → agent-to-agent delegation.

Code blocks marked `rust,no_run` compile against the default features; blocks
that need optional features are marked `rust,ignore` and name the feature.

## 0. Setup

Add the crate (REST + WebSocket serving is on by default):

```toml
[dependencies]
corrosive_agents = "0.1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

Get a **free** NVIDIA API key: visit <https://build.nvidia.com>, open any
model page, press *Get API Key*. Export it:

```sh
export NVIDIA_API_KEY=nvapi-...
```

## 1. Your first agent

Everything starts with the builder. `name` and `version` (valid SemVer) are
required; `build()` validates and returns the immutable agent.

```rust,no_run
use corrosive_agents::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = Agent::builder()
        .name("tutor")
        .version("0.1.0")
        .description("Explains Rust concepts briefly")
        .system_prompt("You are a Rust tutor. Answer in at most four sentences.")
        .model(models::NEMOTRON_3_NANO_30B)
        .llm(NvidiaClient::from_env()?)
        .build()?;

    // History is kept per session id — the second question can say "it".
    let a1 = agent.chat("lesson-1", "What is an Arc<T>?").await?;
    let a2 = agent.chat("lesson-1", "When would I combine it with a Mutex?").await?;
    println!("{a1}\n---\n{a2}");
    Ok(())
}
```

Streaming variant — print tokens as they arrive:

```rust,no_run
use corrosive_agents::prelude::*;
use futures_util::StreamExt;

# async fn run(agent: Agent) -> Result<()> {
let mut stream = agent.chat_stream("lesson-1", "Explain lifetimes").await?;
while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    if chunk.done { break }
    print!("{}", chunk.delta);
}
# Ok(())
# }
```

The NVIDIA client retries 429/5xx with exponential backoff automatically;
tune it with `.with_retry_policy(RetryPolicy { .. })` or disable via
`RetryPolicy::none()`.

## 2. Define the agent in JSON

Ship the agent's *shape* as data. Save as `agent.json`:

```json
{
  "name": "tutor",
  "version": "1.0.0",
  "description": "Explains Rust concepts briefly",
  "model": "nvidia/nemotron-3-nano-30b-a3b",
  "system_prompt": "You are a Rust tutor.",
  "capabilities": [{ "name": "chat", "description": "Q&A" }],
  "skills": ["lookup_doc"],
  "mcp_servers": [
    { "name": "fs", "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] }
  ]
}
```

Load it, then bind code to the declared skill names:

```rust,no_run
use corrosive_agents::prelude::*;
use serde_json::json;

# async fn run() -> Result<()> {
let agent = AgentBuilder::from_json_file("agent.json")?
    .skill(FnSkill::new("lookup_doc", "Looks up a std type", |input| async move {
        Ok(json!({ "url": format!("https://doc.rust-lang.org/std/?search={}",
            input["query"].as_str().unwrap_or_default()) }))
    }))
    .llm(NvidiaClient::from_env()?)
    .build()?;
# Ok(())
# }
```

## 3. Skills and the sandbox

A skill is a named async JSON-in/JSON-out function. Implement the `Skill`
trait for full control, or wrap a closure with `FnSkill`. Give the model a
real schema — it dramatically improves call accuracy:

```rust,no_run
use corrosive_agents::prelude::*;
use serde_json::json;

let weather = FnSkill::new("weather", "Current weather for a city", |input| async move {
    let city = input["city"].as_str().unwrap_or("nowhere").to_string();
    Ok(json!({ "city": city, "temp_c": 21 }))
})
.with_schema(json!({
    "type": "object",
    "properties": { "city": { "type": "string" } },
    "required": ["city"]
}))
.with_permissions(["net"]); // declared requirement
```

The **`SkillPolicy`** is the sandbox: an allowlist of skill names, a set of
granted permissions, and an execution timeout (default 30 s). Skills run on
their own task, so a panic becomes an error instead of killing the agent.

```rust,no_run
use corrosive_agents::prelude::*;
use std::time::Duration;

# fn f(builder: AgentBuilder) -> AgentBuilder {
builder.skill_policy(
    SkillPolicy::new()
        .allow_only(["weather", "lookup_doc"])
        .grant("net")
        .with_timeout(Duration::from_secs(5)),
)
# }
```

## 4. Tool calling — let the model use your skills

`chat_with_tools` offers every registered skill to the model as a function.
When the model asks for a tool, the agent executes it (through the policy!),
feeds the JSON result back, and loops until the model produces text:

```rust,no_run
use corrosive_agents::prelude::*;

# async fn run(agent: Agent) -> Result<()> {
let reply = agent
    .chat_with_tools("session", "What's the weather in Colombo?", 8)
    .await?; // ≤ 8 tool rounds
# Ok(())
# }
```

Over REST the same thing is `POST /chat` with `{"use_tools": true}`.

Track spend with usage hooks — a closure works as an observer:

```rust,no_run
use corrosive_agents::prelude::*;

# fn f(builder: AgentBuilder) -> AgentBuilder {
builder.usage_observer(|event: &UsageEvent| {
    println!("[{}] {} used {} tokens", event.session_id, event.model, event.usage.total_tokens);
})
# }
```

`agent.usage()` returns cumulative totals at any time.

## 5. Memory: sessions and RAG

**Sessions** live in a `SessionStore`. The default is in-memory; swap in
SQLite or Redis for persistence across restarts (features `sqlite-sessions`
/ `redis-sessions`):

```rust,ignore
// --features sqlite-sessions
let agent = Agent::builder()
    .name("persistent").version("1.0.0")
    .llm(NvidiaClient::from_env()?)
    .session_store(SqliteSessionStore::open("sessions.db")?)
    .build()?;
// After a restart, session ids resume where they left off.
```

**RAG** needs an embedding provider (the `NvidiaClient` doubles as one) and a
`VectorStore` — in-memory, Qdrant (`qdrant`), Pinecone (`pinecone`),
Postgres/pgvector (`pgvector`), or your own trait impl:

```rust,no_run
use corrosive_agents::prelude::*;
use serde_json::json;

# async fn run() -> Result<()> {
let nvidia = NvidiaClient::from_env()?;
let agent = Agent::builder()
    .name("librarian").version("0.1.0")
    .llm(nvidia.clone())
    .embeddings(nvidia)
    .vector_store(InMemoryVectorStore::new())
    .build()?;

// Index — one fact, a batch, or a whole document (auto-chunked):
agent.remember("Nemotron 3 Nano is fast.", json!({"topic": "models"})).await?;
agent.remember_document(&std::fs::read_to_string("guide.md")?,
    json!({"source": "guide"}), 1200, 200).await?;

// Retrieve — optionally filtered by metadata:
let hits = agent.recall("which model is fast?", 3).await?;
let guide_only = agent
    .recall_filtered("chunking", 3, &MetadataFilter::new().eq("source", json!("guide")))
    .await?;
# Ok(())
# }
```

## 6. Identity, verification, and trust

Give the agent an Ed25519 identity and the manifest is signed at `build()`:

```rust,no_run
use corrosive_agents::prelude::*;

# fn main() -> Result<()> {
let agent = Agent::builder()
    .name("trusted").version("1.0.0")
    .generate_identity()
    .build()?;

let manifest_json = agent.manifest().to_json()?;   // ship this anywhere
let did = agent.identity().unwrap().did_key();     // did:key:z6Mk…

// Any consumer, offline:
let received = AgentManifest::from_json(&manifest_json)?;
received.verify()?;            // embedded key
received.verify_with(&did)?;   // or pinned key / DID
# Ok(())
# }
```

Store the secret (`identity.secret_key_base64()`) somewhere safe and restore
with `AgentIdentity::from_secret_base64` to keep the same identity across
releases.

**Rotation & revocation** — replace a key without losing consumer trust:

```rust,no_run
use corrosive_agents::prelude::*;

# fn main() -> Result<()> {
let old = AgentIdentity::generate();
let new = AgentIdentity::generate();
let mut manifest = AgentManifest::new("trusted", "1.1.0");
manifest.sign(&old)?;
manifest.rotate_identity(&old, &new)?; // old key endorses new; re-signed

let mut trust = TrustStore::new();
trust.trust(&old.public_key_base64())?;         // consumer pinned the OLD key
trust.verify_manifest(&manifest)?;              // still verifies via the chain
trust.revoke(Revocation::create(&old, "rotated away"))?; // kills post-revocation use
# Ok(())
# }
```

**X.509** (feature `x509`) — for PKI-speaking integrations:

```rust,ignore
// --features x509
let cert_pem = corrosive_agents::x509::generate_certificate_pem(&identity, "trusted")?;
corrosive_agents::x509::verify_manifest_with_certificate(&manifest, &cert_pem)?;
```

## 7. MCP: tools, resources, prompts

Declare servers in the manifest (stdio) or connect over streamable HTTP:

```rust,no_run
use corrosive_agents::prelude::*;
use serde_json::json;

# async fn run(agent: Agent) -> Result<()> {
agent.connect_mcp_servers().await?;             // manifest-declared servers

let tools = agent.mcp_tools("fs").await?;
let listing = agent.call_mcp_tool("fs", "list_directory", json!({"path": "/tmp"})).await?;

// Or an HTTP/SSE server, ad hoc:
let remote = McpClient::connect(
    &McpServerConfig::http("docs", "https://mcp.example.com/mcp")
        .with_header("Authorization", "Bearer token"),
).await?;
let resources = remote.list_resources().await?;
let readme = remote.read_resource("file:///README.md").await?;
let prompts = remote.list_prompts().await?;
# Ok(())
# }
```

## 8. Serving: REST, WebSocket, gRPC

```rust,ignore
// feature `server` (on by default; marked ignore so the tutorial also
// compiles under --no-default-features)
use std::sync::Arc;
use corrosive_agents::auth::AuthScheme;
use corrosive_agents::prelude::*;
use corrosive_agents::server;

# async fn run(agent: Agent) -> Result<()> {
let agent = Arc::new(agent);

// Open server with graceful shutdown on Ctrl-C/SIGTERM:
server::serve_with_shutdown(
    agent.clone(),
    "0.0.0.0:8080".parse().unwrap(),
    server::shutdown_signal(),
).await?;

// API-key or JWT protected (probes stay open):
server::serve_with_auth(
    agent,
    "0.0.0.0:8080".parse().unwrap(),
    AuthScheme::jwt_hs256("shared-secret").with_issuer("my-platform"),
).await?;
# Ok(())
# }
```

For an identity provider (Auth0, Keycloak, …), verify RS256 tokens against
its JWKS instead of sharing a secret:

```rust,ignore
// feature `server` (or `grpc`)
use corrosive_agents::auth::{AuthScheme, JwksStore};

let jwks = JwksStore::from_url("https://idp.example.com/.well-known/jwks.json").await?;
let auth = AuthScheme::jwt_rs256(jwks)
    .with_issuer("https://idp.example.com/")
    .with_audience("corrosive-agents");
```

(`JwksStore::refresh()` re-fetches keys — call it from a periodic task if
your provider rotates them.)

- `/health` is liveness; `/ready` is readiness — flip it with
  `agent.set_ready(false)` while warming up or draining.
- With feature `openapi`, the full spec is served at `/openapi.json`.
- With feature `tls`: `server::serve_tls(agent, addr, &TlsConfig::from_pem_files("cert.pem", "key.pem"))`.
- gRPC (feature `grpc`) mirrors all of it: `grpc::serve`, `grpc::serve_with_auth`,
  `grpc::serve_with_shutdown`, `grpc::serve_tls` — clients authenticate with
  `authorization` / `x-api-key` metadata.

WebSocket protocol (at `/ws`): send
`{"type":"chat","message":"hi","stream":true}`, receive `chunk` events and a
final `done`.

## 9. Agent-to-agent (A2A) delegation

Compose agents across processes/machines. Pin the peer's DID and the client
verifies its signed manifest before the first request:

```rust,no_run
use corrosive_agents::prelude::*;
use serde_json::json;

# async fn run() -> Result<()> {
let orchestrator = Agent::builder()
    .name("orchestrator").version("1.0.0")
    .peer("cruncher",
        RemoteAgent::new("http://worker:8080")
            .with_pinned_key("did:key:z6Mk...worker..."))
    .build()?;

let reply = orchestrator.delegate_chat("cruncher", "job-7", "crunch this").await?;
let out = orchestrator.delegate_skill("cruncher", "fibonacci", json!({"n": 42})).await?;
# Ok(())
# }
```

A mismatched identity fails **before** any payload is sent. Run
`cargo run --example a2a_delegation` to see it end to end, offline.

## 10. Production checklist

- [ ] Persist the identity secret (vault/KMS), not in the manifest or repo.
- [ ] Pin peer keys or DIDs for every `RemoteAgent`.
- [ ] `SkillPolicy` with an explicit allowlist + timeouts on any deployment
      that loads third-party skills.
- [ ] Persistent `SessionStore` (SQLite single node, Redis multi-node).
- [ ] `serve_with_auth` + `tls` (or terminate TLS at your ingress).
- [ ] Wire `usage_observer` into your metrics stack.
- [ ] `/ready` in your orchestrator's readiness probe;
      `serve_with_shutdown(…, shutdown_signal())` for clean rollouts.
- [ ] Watch NVIDIA model availability — catalog entries can 404; probe with a
      1-token completion and keep model ids configurable via the manifest.

Every topic above has a runnable example in `examples/` — start with
`build_agent`, `vector_rag`, `sign_and_verify`, and `a2a_delegation`.
