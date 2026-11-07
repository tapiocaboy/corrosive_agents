# Changelog

All notable changes to `corrosive_agents` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.2] - 2026-11-07

### Added

- Decentralised multi-agent mesh use case (`usecases/decentralised_agents`):
  a client, an orchestrator, and four specialists cooperating over libp2p
  (mDNS + gossipsub) with Ed25519-signed, discovery-pinned messages, driven by
  real NVIDIA NIM calls. Shipped as a repo example, excluded from the published
  crate via `Cargo.toml`.

### Changed

- **NVIDIA NIM model catalog refreshed** (`llm::models`). Several Nemotron ids
  reached end-of-life upstream and now return `404`/`410`, including the
  previous client default. Added currently-callable free ids —
  `NEMOTRON_35_BOLT_E4B`, `NEMOTRON_35_LIGHTNING_30B`, `NEMOTRON_35_SUPER_VL_120B`,
  `DEEPSEEK_V41_FLASH`, `MINIMAX_M31_FLASH`, and embeddings `NEMOTRON_35_EMBED_1B`
  / `NEMOTRON_35_EMBED_8B` (all verified callable on a free key, 2026-11-07).
- `NvidiaClient`'s default chat model is now `nvidia/nemotron-3.5-bolt-e4b` and
  its default embedding model `nvidia/nemotron-3.5-embed-1b`, so a fresh free
  key works out of the box without an explicit `.with_model(...)`.
- Documentation refreshed to the live catalog: the crate README, the tutorial
  (`docs/TUTORIAL.md`), the runnable `examples/`, and API doc comments
  (`AgentManifest`, `EmbeddingProvider`, `PgVectorStore`) now reference the
  current model ids instead of the retired ones.

### Deprecated

- The retired model-id constants (`NEMOTRON_3_NANO_30B`, `NEMOTRON_3_SUPER_120B`,
  `NEMOTRON_3_ULTRA_550B`, `LLAMA_NEMOTRON_SUPER_49B`, `LLAMA_NEMOTRON_ULTRA_253B`,
  `LLAMA_NEMOTRON_NANO_8B`, `NEMOTRON_MINI_4B`, `NEMOTRON_NANO_9B_V2`,
  `EMBED_QA_E5_V5`, `NEMOTRON_EMBED_1B_V2`) are kept for source compatibility
  but marked `#[deprecated]` — they return `404`/`410` on NVIDIA NIM.

### Security

- Refreshed `Cargo.lock` to drop a yanked `chacha20` and pull patched releases
  of `tokio`, `rustls`, `rustls-webpki`, `h2`, `webpki-roots`, and `ctutils`,
  clearing all outstanding RustSec / `cargo-deny` advisories. MSRV (1.88) is
  unchanged.

## [0.0.1] - 2026-07-06

Initial release.

### Added

- **Agent core** — builder-pattern construction with SemVer validation;
  JSON manifests (name, version, capabilities, skills, MCP servers);
  per-session conversation history.
- **NVIDIA Nemotron LLM client** — chat, SSE streaming, tool calling, and
  embeddings against the free NIM API (`integrate.api.nvidia.com`), with
  exponential-backoff retries honoring `Retry-After`.
- **Tool-calling loop** — `Agent::chat_with_tools` auto-invokes registered
  skills from model tool calls, bounded rounds, policy-enforced.
- **Skills** — async JSON-in/JSON-out abilities with a sandbox
  (`SkillPolicy`): allowlists, permission grants, timeouts, panic isolation.
- **MCP client** — stdio and streamable-HTTP/SSE transports; tools,
  resources, and prompts.
- **Verifiable identity** — Ed25519-signed manifests; `did:key` DIDs;
  X.509 certificate option (feature `x509`); key rotation proofs and
  self-signed revocations enforced by a `TrustStore`.
- **Transports** — REST + WebSocket (axum) and gRPC (tonic, vendored
  protobuf codegen — no `protoc` needed), with API-key / JWT HS256 /
  JWT RS256+JWKS authentication, TLS helpers (feature `tls`), graceful
  shutdown, `/health` + `/ready` probes, and an OpenAPI 3 document
  (feature `openapi`).
- **Agent-to-agent delegation** — `RemoteAgent` peers verified against
  pinned keys/DIDs before any delegated chat or skill call.
- **Session stores** — in-memory (default), SQLite
  (feature `sqlite-sessions`), and Redis (feature `redis-sessions`).
- **Vector stores** — in-memory, Qdrant, Pinecone, and PostgreSQL/pgvector
  backends behind one trait; metadata filtering, text chunking, batched
  upserts, and `remember`/`recall` RAG helpers on the agent.
- **Token usage accounting** — `UsageObserver` hooks plus built-in totals.
- **Docs** — a step-by-step tutorial (`docs/TUTORIAL.md`), shipped on
  docs.rs as the `tutorial` module with compile-checked examples.

[Unreleased]: https://github.com/tapiocaboy/corrosive_agents/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/tapiocaboy/corrosive_agents/releases/tag/v0.0.1
