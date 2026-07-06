//! The JSON manifest describing an agent, and its capability entries.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::identity::{verify_signature, AgentIdentity};
use crate::mcp::McpServerConfig;

/// One thing an agent can do, as declared in its manifest.
///
/// Capabilities are declarative metadata: they describe the agent to humans
/// and other agents, and can carry free-form JSON configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Capability {
    /// Machine-readable capability name, e.g. `"chat"` or `"rag"`.
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Whether the capability is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional free-form configuration for this capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub config: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

impl Capability {
    /// Create an enabled capability with a name and description.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            enabled: true,
            config: None,
        }
    }

    /// Attach free-form JSON configuration.
    #[must_use]
    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = Some(config);
        self
    }

    /// Mark the capability as disabled.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// The signed, JSON-serializable description of an agent.
///
/// A manifest can be loaded from a JSON file, signed with an
/// [`AgentIdentity`], published, and later verified by anyone holding only
/// the manifest itself (the public key travels inside it).
///
/// # Example manifest
///
/// ```json
/// {
///   "name": "research-agent",
///   "version": "1.0.0",
///   "description": "Answers research questions",
///   "model": "nvidia/llama-3.3-nemotron-super-49b-v1",
///   "system_prompt": "You are a concise research assistant.",
///   "capabilities": [
///     { "name": "chat", "description": "Conversational Q&A" }
///   ],
///   "skills": ["summarize"],
///   "mcp_servers": [
///     { "name": "fs", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentManifest {
    /// Agent name (required, non-empty).
    pub name: String,
    /// Agent version, e.g. `"1.0.0"` (required, non-empty).
    pub version: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Declared capabilities.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Default LLM model id, e.g. `"nvidia/llama-3.3-nemotron-super-49b-v1"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// System prompt prepended to every conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Names of skills this agent declares (implementations are registered
    /// through [`AgentBuilder::skill`](crate::agent::AgentBuilder::skill)).
    #[serde(default)]
    pub skills: Vec<String>,
    /// MCP servers the agent may connect to.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Base64 Ed25519 public key of the signing identity (set by [`sign`](Self::sign)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// The signing identity as a W3C `did:key` DID (set by [`sign`](Self::sign)).
    /// Always corresponds to `public_key`; [`verify`](Self::verify) checks the pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub did: Option<String>,
    /// Chain of key rotations (oldest first) ending at `public_key` — see
    /// [`rotate_identity`](Self::rotate_identity) and [`crate::trust`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_history: Vec<crate::trust::RotationProof>,
    /// Base64 Ed25519 signature over the canonical manifest JSON
    /// (set by [`sign`](Self::sign)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl AgentManifest {
    /// Create a minimal manifest with a name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            ..Default::default()
        }
    }

    /// Parse a manifest from a JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    /// Load a manifest from a JSON file on disk.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        Self::from_json(&contents)
    }

    /// Serialize the manifest to pretty-printed JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Write the manifest as JSON to a file.
    pub fn to_json_file(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_json()?)?;
        Ok(())
    }

    /// The canonical byte representation that gets signed: the manifest with
    /// `signature` cleared, serialized as JSON.
    ///
    /// This is deterministic because struct fields serialize in declaration
    /// order and `serde_json` maps are ordered (`BTreeMap`) by default.
    pub fn signable_bytes(&self) -> Result<Vec<u8>> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        Ok(serde_json::to_vec(&unsigned)?)
    }

    /// Sign the manifest: embeds the identity's public key (and its `did:key`
    /// DID) plus a signature over the canonical JSON.
    pub fn sign(&mut self, identity: &AgentIdentity) -> Result<()> {
        self.public_key = Some(identity.public_key_base64());
        self.did = Some(identity.did_key());
        let bytes = self.signable_bytes()?;
        self.signature = Some(identity.sign(&bytes));
        Ok(())
    }

    /// Rotate the agent's identity: the old key signs a
    /// [`RotationProof`](crate::trust::RotationProof) endorsing the new key,
    /// the proof is appended to `key_history`, and the manifest is re-signed
    /// with the new identity.
    ///
    /// Consumers who pinned the old key keep trusting the agent via
    /// [`TrustStore::verify_manifest`](crate::trust::TrustStore::verify_manifest).
    pub fn rotate_identity(&mut self, old: &AgentIdentity, new: &AgentIdentity) -> Result<()> {
        if self.public_key.as_deref() != Some(old.public_key_base64().as_str()) {
            return Err(Error::Identity(
                "rotation must start from the manifest's current key".into(),
            ));
        }
        self.key_history
            .push(crate::trust::RotationProof::create(old, new));
        self.sign(new)
    }

    /// Verify the manifest against its embedded public key.
    ///
    /// Fails if the manifest is unsigned, the key/signature are malformed,
    /// any signed field was modified after signing, or the embedded `did`
    /// does not match `public_key`.
    pub fn verify(&self) -> Result<()> {
        let public_key = self
            .public_key
            .as_deref()
            .ok_or_else(|| Error::Verification("manifest has no public key".into()))?;
        if let Some(did) = &self.did {
            if crate::identity::public_key_from_did(did)? != public_key {
                return Err(Error::Verification(
                    "manifest did does not match its public key".into(),
                ));
            }
        }
        self.verify_with(public_key)
    }

    /// Verify the manifest against an externally supplied key — base64 or
    /// `did:key:` DID. Use this when you already know which identity the
    /// agent *should* have.
    pub fn verify_with(&self, key_or_did: &str) -> Result<()> {
        let signature = self
            .signature
            .as_deref()
            .ok_or_else(|| Error::Verification("manifest is not signed".into()))?;
        verify_signature(key_or_did, &self.signable_bytes()?, signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AgentManifest {
        let mut m = AgentManifest::new("test-agent", "1.2.3");
        m.description = "A test agent".into();
        m.capabilities
            .push(Capability::new("chat", "Talks").with_config(serde_json::json!({"k": 1})));
        m.skills.push("echo".into());
        m
    }

    #[test]
    fn json_roundtrip() {
        let manifest = sample();
        let json = manifest.to_json().unwrap();
        let parsed = AgentManifest::from_json(&json).unwrap();
        assert_eq!(manifest, parsed);
    }

    #[test]
    fn sign_then_verify() {
        let identity = AgentIdentity::generate();
        let mut manifest = sample();
        manifest.sign(&identity).unwrap();
        manifest.verify().unwrap();
        manifest.verify_with(&identity.public_key_base64()).unwrap();
    }

    #[test]
    fn tampering_breaks_verification() {
        let identity = AgentIdentity::generate();
        let mut manifest = sample();
        manifest.sign(&identity).unwrap();
        manifest.version = "9.9.9".into();
        assert!(manifest.verify().is_err());
    }

    #[test]
    fn verification_survives_json_roundtrip() {
        let identity = AgentIdentity::generate();
        let mut manifest = sample();
        manifest.sign(&identity).unwrap();
        let reparsed = AgentManifest::from_json(&manifest.to_json().unwrap()).unwrap();
        reparsed.verify().unwrap();
    }

    #[test]
    fn unsigned_manifest_fails_verification() {
        assert!(sample().verify().is_err());
    }

    #[test]
    fn capability_defaults_enabled() {
        let cap: Capability = serde_json::from_str(r#"{"name": "chat"}"#).unwrap();
        assert!(cap.enabled);
        assert!(cap.description.is_empty());
    }

    #[test]
    fn signing_embeds_matching_did() {
        let identity = AgentIdentity::generate();
        let mut manifest = sample();
        manifest.sign(&identity).unwrap();
        assert_eq!(manifest.did, Some(identity.did_key()));
        manifest.verify().unwrap();
        // Verification also works when pinning by DID instead of base64.
        manifest.verify_with(&identity.did_key()).unwrap();

        // A mismatched DID is rejected even with an intact signature chain.
        manifest.did = Some(AgentIdentity::generate().did_key());
        assert!(manifest.verify().is_err());
    }

    #[test]
    fn rotate_identity_re_signs_and_records_history() {
        let old = AgentIdentity::generate();
        let new = AgentIdentity::generate();
        let mut manifest = sample();
        manifest.sign(&old).unwrap();

        manifest.rotate_identity(&old, &new).unwrap();
        assert_eq!(manifest.public_key, Some(new.public_key_base64()));
        assert_eq!(manifest.key_history.len(), 1);
        manifest.verify().unwrap();
        manifest.key_history[0].verify().unwrap();

        // Rotating from a key that is not current is refused.
        let stranger = AgentIdentity::generate();
        assert!(manifest.rotate_identity(&stranger, &old).is_err());
    }
}
