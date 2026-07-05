//! Agent-to-agent (A2A) delegation.
//!
//! A [`RemoteAgent`] is a client for another corrosive agent's REST API
//! (`/agent`, `/agent/manifest`, `/chat`, `/skills/{name}`). Register peers
//! on an [`Agent`](crate::agent::Agent) — via
//! [`AgentBuilder::peer`](crate::agent::AgentBuilder::peer) or
//! [`Agent::add_peer`](crate::agent::Agent::add_peer) — and delegate work:
//!
//! ```no_run
//! use corrosive_agents::prelude::*;
//! use serde_json::json;
//!
//! # async fn run() -> corrosive_agents::Result<()> {
//! let orchestrator = Agent::builder()
//!     .name("orchestrator")
//!     .version("1.0.0")
//!     .peer(
//!         "researcher",
//!         RemoteAgent::new("http://research-agent:8080")
//!             // refuse to delegate unless the peer proves this identity:
//!             .with_pinned_key("did:key:z6Mk..."),
//!     )
//!     .build()?;
//!
//! let answer = orchestrator
//!     .delegate_chat("researcher", "job-42", "Summarize the findings")
//!     .await?;
//! let out = orchestrator
//!     .delegate_skill("researcher", "extract", json!({ "url": "…" }))
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! When a pinned key (base64 or `did:key:`) is set, the peer's signed
//! manifest is fetched and verified against it before the first delegation;
//! the result is cached for the lifetime of the [`RemoteAgent`].

use serde_json::{json, Value};
use tokio::sync::OnceCell;

use crate::agent::{AgentInfo, AgentManifest};
use crate::error::{Error, Result};
use crate::identity::normalize_key;

/// A client for a remote corrosive agent served over REST.
#[derive(Debug)]
pub struct RemoteAgent {
    http: reqwest::Client,
    base_url: String,
    pinned_key: Option<String>,
    trusted: OnceCell<()>,
}

impl RemoteAgent {
    /// Point at a peer's REST API base URL (e.g. `http://host:8080`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            pinned_key: None,
            trusted: OnceCell::new(),
        }
    }

    /// Require the peer to prove this identity (base64 public key or
    /// `did:key:` DID) before any delegation. The peer's signed manifest is
    /// fetched from `/agent/manifest` and verified on first use.
    #[must_use]
    pub fn with_pinned_key(mut self, key_or_did: impl Into<String>) -> Self {
        self.pinned_key = Some(key_or_did.into());
        self
    }

    /// The peer's base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn parse_response<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        if !status.is_success() {
            let body: Value = response.json().await.unwrap_or(Value::Null);
            let detail = body
                .get("error")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| body.to_string());
            return Err(Error::A2a(format!("peer returned {status}: {detail}")));
        }
        Ok(response.json().await?)
    }

    /// Fetch the peer's public info (`GET /agent`).
    pub async fn info(&self) -> Result<AgentInfo> {
        let response = self
            .http
            .get(format!("{}/agent", self.base_url))
            .send()
            .await?;
        Self::parse_response(response).await
    }

    /// Fetch the peer's signed manifest (`GET /agent/manifest`).
    pub async fn manifest(&self) -> Result<AgentManifest> {
        let response = self
            .http
            .get(format!("{}/agent/manifest", self.base_url))
            .send()
            .await?;
        Self::parse_response(response).await
    }

    /// Fetch and verify the peer's manifest. Checks the signature, and — when
    /// a key is pinned — that the manifest's identity matches it.
    pub async fn verify(&self) -> Result<AgentManifest> {
        let manifest = self.manifest().await?;
        manifest.verify()?;
        if let Some(pinned) = &self.pinned_key {
            let expected = normalize_key(pinned)?;
            if manifest.public_key.as_deref() != Some(expected.as_str()) {
                return Err(Error::A2a(format!(
                    "peer at {} presented a different identity than the pinned key",
                    self.base_url
                )));
            }
        }
        Ok(manifest)
    }

    /// Verify the peer once (only when a key is pinned), caching the result.
    async fn ensure_trusted(&self) -> Result<()> {
        if self.pinned_key.is_none() {
            return Ok(());
        }
        self.trusted
            .get_or_try_init(|| async {
                self.verify().await?;
                Ok::<(), Error>(())
            })
            .await?;
        Ok(())
    }

    /// One chat turn on the peer (`POST /chat`). Session history lives on the
    /// peer, keyed by `session_id`.
    pub async fn chat(&self, session_id: &str, message: &str) -> Result<String> {
        self.ensure_trusted().await?;
        let response = self
            .http
            .post(format!("{}/chat", self.base_url))
            .json(&json!({ "session_id": session_id, "message": message }))
            .send()
            .await?;
        let body: Value = Self::parse_response(response).await?;
        body.get("reply")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| Error::A2a("peer chat response had no 'reply'".into()))
    }

    /// Execute a skill on the peer (`POST /skills/{name}`).
    pub async fn execute_skill(&self, name: &str, input: Value) -> Result<Value> {
        self.ensure_trusted().await?;
        let response = self
            .http
            .post(format!("{}/skills/{name}", self.base_url))
            .json(&input)
            .send()
            .await?;
        Self::parse_response(response).await
    }
}
