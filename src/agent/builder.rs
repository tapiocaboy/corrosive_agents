//! Fluent construction of [`Agent`]s (Builder pattern).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::a2a::RemoteAgent;
use crate::agent::{Agent, AgentManifest, Capability};
use crate::error::{Error, Result};
use crate::identity::AgentIdentity;
use crate::llm::{EmbeddingProvider, LlmProvider};
use crate::mcp::McpServerConfig;
use crate::session::{InMemorySessionStore, SessionStore};
use crate::skills::{Skill, SkillRegistry};
use crate::vector::VectorStore;

/// Builds an [`Agent`] step by step.
///
/// Obtain one with [`Agent::builder`], chain configuration calls, and finish
/// with [`build`](Self::build). If the builder holds an identity (via
/// [`identity`](Self::identity) or [`generate_identity`](Self::generate_identity)),
/// the manifest is automatically signed during `build`.
///
/// ```
/// use corrosive_agents::prelude::*;
///
/// let agent = Agent::builder()
///     .name("greeter")
///     .version("0.1.0")
///     .capability(Capability::new("chat", "Says hello"))
///     .generate_identity()
///     .build()
///     .unwrap();
/// assert!(agent.verify().is_ok());
/// ```
#[derive(Default)]
pub struct AgentBuilder {
    manifest: AgentManifest,
    identity: Option<AgentIdentity>,
    generate_identity: bool,
    llm: Option<Arc<dyn LlmProvider>>,
    embeddings: Option<Arc<dyn EmbeddingProvider>>,
    vector_store: Option<Arc<dyn VectorStore>>,
    skills: Vec<Arc<dyn Skill>>,
    session_store: Option<Arc<dyn SessionStore>>,
    peers: HashMap<String, Arc<RemoteAgent>>,
}

impl AgentBuilder {
    /// Create an empty builder (equivalent to [`Agent::builder`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from an existing manifest (e.g. loaded from JSON).
    /// Later builder calls override manifest fields.
    pub fn from_manifest(manifest: AgentManifest) -> Self {
        Self {
            manifest,
            ..Self::default()
        }
    }

    /// Start from a JSON manifest string.
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(Self::from_manifest(AgentManifest::from_json(json)?))
    }

    /// Start from a JSON manifest file.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::from_manifest(AgentManifest::from_json_file(path)?))
    }

    /// Set the agent name (required).
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.manifest.name = name.into();
        self
    }

    /// Set the agent version (required), e.g. `"1.0.0"`.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.manifest.version = version.into();
        self
    }

    /// Set the human-readable description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.manifest.description = description.into();
        self
    }

    /// Set the default LLM model id (see [`crate::llm::models`]).
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.manifest.model = Some(model.into());
        self
    }

    /// Set the system prompt prepended to every conversation.
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.manifest.system_prompt = Some(prompt.into());
        self
    }

    /// Declare a capability.
    #[must_use]
    pub fn capability(mut self, capability: Capability) -> Self {
        self.manifest.capabilities.push(capability);
        self
    }

    /// Declare an MCP server the agent can connect to.
    #[must_use]
    pub fn mcp_server(mut self, config: McpServerConfig) -> Self {
        self.manifest.mcp_servers.push(config);
        self
    }

    /// Register a skill implementation.
    #[must_use]
    pub fn skill(mut self, skill: impl Skill + 'static) -> Self {
        self.skills.push(Arc::new(skill));
        self
    }

    /// Register an already-shared skill implementation.
    #[must_use]
    pub fn skill_arc(mut self, skill: Arc<dyn Skill>) -> Self {
        self.skills.push(skill);
        self
    }

    /// Set the LLM provider (e.g. [`crate::llm::NvidiaClient`]).
    #[must_use]
    pub fn llm(mut self, provider: impl LlmProvider + 'static) -> Self {
        self.llm = Some(Arc::new(provider));
        self
    }

    /// Set the embedding provider used by `remember`/`recall`.
    #[must_use]
    pub fn embeddings(mut self, provider: impl EmbeddingProvider + 'static) -> Self {
        self.embeddings = Some(Arc::new(provider));
        self
    }

    /// Set the vector store used by `remember`/`recall`.
    #[must_use]
    pub fn vector_store(mut self, store: impl VectorStore + 'static) -> Self {
        self.vector_store = Some(Arc::new(store));
        self
    }

    /// Set the session store backing conversation history. Defaults to
    /// [`InMemorySessionStore`]; use
    /// [`SqliteSessionStore`](crate::session::SqliteSessionStore) or
    /// [`RedisSessionStore`](crate::session::RedisSessionStore) for
    /// persistence across restarts.
    #[must_use]
    pub fn session_store(mut self, store: impl SessionStore + 'static) -> Self {
        self.session_store = Some(Arc::new(store));
        self
    }

    /// Register a peer agent for A2A delegation under a local name
    /// (see [`crate::a2a`]).
    #[must_use]
    pub fn peer(mut self, name: impl Into<String>, peer: RemoteAgent) -> Self {
        self.peers.insert(name.into(), Arc::new(peer));
        self
    }

    /// Give the agent an existing identity. The manifest will be signed with
    /// it during [`build`](Self::build).
    #[must_use]
    pub fn identity(mut self, identity: AgentIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Generate a fresh Ed25519 identity during [`build`](Self::build).
    #[must_use]
    pub fn generate_identity(mut self) -> Self {
        self.generate_identity = true;
        self
    }

    /// Validate the configuration and construct the [`Agent`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when `name` is empty or `version` is not
    /// valid [SemVer](https://semver.org) (e.g. `"1.0.0"`, `"2.1.0-beta.1"`),
    /// and propagates signing errors when an identity is present.
    pub fn build(mut self) -> Result<Agent> {
        if self.manifest.name.trim().is_empty() {
            return Err(Error::Config("agent name is required".into()));
        }
        if self.manifest.version.trim().is_empty() {
            return Err(Error::Config("agent version is required".into()));
        }
        semver::Version::parse(self.manifest.version.trim()).map_err(|e| {
            Error::Config(format!(
                "agent version '{}' is not valid semver: {e}",
                self.manifest.version
            ))
        })?;

        let mut registry = SkillRegistry::new();
        for skill in self.skills {
            let name = skill.name().to_string();
            registry.register(skill);
            if !self.manifest.skills.contains(&name) {
                self.manifest.skills.push(name);
            }
        }

        let identity = match (self.identity, self.generate_identity) {
            (Some(identity), _) => Some(identity),
            (None, true) => Some(AgentIdentity::generate()),
            (None, false) => None,
        };
        if let Some(identity) = &identity {
            self.manifest.sign(identity)?;
        }

        Ok(Agent {
            manifest: self.manifest,
            identity,
            llm: self.llm,
            embeddings: self.embeddings,
            vector_store: self.vector_store,
            skills: registry,
            mcp_clients: RwLock::new(HashMap::new()),
            sessions: self
                .session_store
                .unwrap_or_else(|| Arc::new(InMemorySessionStore::new())),
            peers: RwLock::new(self.peers),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::FnSkill;
    use serde_json::json;

    #[test]
    fn build_requires_name_and_version() {
        assert!(matches!(Agent::builder().build(), Err(Error::Config(_))));
        assert!(matches!(
            Agent::builder().name("x").build(),
            Err(Error::Config(_))
        ));
        assert!(Agent::builder().name("x").version("0.1.0").build().is_ok());
    }

    #[test]
    fn build_validates_semver() {
        for bad in ["1", "1.0", "v1.0.0", "abc", "1.0.0.0"] {
            let err = Agent::builder().name("x").version(bad).build().unwrap_err();
            assert!(
                matches!(&err, Error::Config(msg) if msg.contains("semver")),
                "expected semver error for '{bad}', got: {err}"
            );
        }
        for good in ["1.0.0", "0.1.0", "2.1.0-beta.1", "1.0.0+build.5"] {
            assert!(
                Agent::builder().name("x").version(good).build().is_ok(),
                "'{good}' should be accepted"
            );
        }
    }

    #[test]
    fn identity_signs_manifest_at_build() {
        let agent = Agent::builder()
            .name("signed")
            .version("1.0.0")
            .generate_identity()
            .build()
            .unwrap();
        agent.verify().unwrap();
        assert!(agent.manifest().public_key.is_some());
    }

    #[test]
    fn registered_skills_appear_in_manifest() {
        let agent = Agent::builder()
            .name("skilled")
            .version("1.0.0")
            .skill(FnSkill::new("echo", "Echoes input", |input| async move {
                Ok(input)
            }))
            .build()
            .unwrap();
        assert_eq!(agent.manifest().skills, vec!["echo".to_string()]);
        assert!(agent.skills().get("echo").is_some());
    }

    #[tokio::test]
    async fn chat_without_llm_is_a_clear_error() {
        let agent = Agent::builder()
            .name("mute")
            .version("1.0.0")
            .build()
            .unwrap();
        let err = agent.chat("s", "hi").await.unwrap_err();
        assert!(matches!(err, Error::NotConfigured("LLM provider")));
    }

    #[tokio::test]
    async fn skills_execute_through_the_agent() {
        let agent = Agent::builder()
            .name("skilled")
            .version("1.0.0")
            .skill(FnSkill::new("add", "Adds a and b", |input| async move {
                let sum = input["a"].as_i64().unwrap_or(0) + input["b"].as_i64().unwrap_or(0);
                Ok(json!({ "sum": sum }))
            }))
            .build()
            .unwrap();
        let out = agent
            .execute_skill("add", json!({ "a": 2, "b": 40 }))
            .await
            .unwrap();
        assert_eq!(out["sum"], 42);
    }
}
