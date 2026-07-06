//! Key rotation, revocation, and trust decisions for agent identities.
//!
//! # Rotation
//!
//! An agent replaces its key by having the **old key endorse the new one** in
//! a signed [`RotationProof`]. Proofs accumulate in the manifest's
//! `key_history` (oldest first), forming a chain from any earlier key to the
//! current one. A consumer who pinned an old key can still verify the agent
//! by walking the chain ([`TrustStore::verify_manifest`]).
//!
//! # Revocation
//!
//! A [`Revocation`] is a self-signed statement that a key must no longer be
//! trusted. Rotations *made before* the revocation timestamp remain valid
//! (so you can rotate away from a compromised key and then revoke it);
//! anything signed by the key *after* revocation is rejected. Timestamps are
//! self-asserted — distribute revocations promptly and out-of-band.
//!
//! ```
//! use corrosive_agents::identity::AgentIdentity;
//! use corrosive_agents::trust::{RotationProof, TrustStore};
//! use corrosive_agents::agent::AgentManifest;
//!
//! # fn main() -> corrosive_agents::Result<()> {
//! let old = AgentIdentity::generate();
//! let new = AgentIdentity::generate();
//!
//! let mut manifest = AgentManifest::new("agent", "1.0.0");
//! manifest.sign(&old)?;
//! manifest.rotate_identity(&old, &new)?; // re-signed by `new`, chain recorded
//!
//! let mut trust = TrustStore::new();
//! trust.trust(&old.public_key_base64())?;      // consumer pinned the old key…
//! trust.verify_manifest(&manifest)?;           // …and still accepts the agent
//! # Ok(())
//! # }
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::agent::AgentManifest;
use crate::error::{Error, Result};
use crate::identity::{normalize_key, verify_signature, AgentIdentity};

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A signed endorsement of a new key by the previous key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RotationProof {
    /// Base64 public key being rotated away from.
    pub previous_key: String,
    /// Base64 public key being rotated to.
    pub new_key: String,
    /// Unix timestamp (seconds) of the rotation.
    pub rotated_at: u64,
    /// Base64 Ed25519 signature by `previous_key` over the rotation message.
    pub signature: String,
}

impl RotationProof {
    fn message(previous_key: &str, new_key: &str, rotated_at: u64) -> Vec<u8> {
        format!("corrosive-rotation:{previous_key}:{new_key}:{rotated_at}").into_bytes()
    }

    /// Create a proof: `old` endorses `new` as its successor.
    pub fn create(old: &AgentIdentity, new: &AgentIdentity) -> Self {
        let previous_key = old.public_key_base64();
        let new_key = new.public_key_base64();
        let rotated_at = unix_now();
        let signature = old.sign(&Self::message(&previous_key, &new_key, rotated_at));
        Self {
            previous_key,
            new_key,
            rotated_at,
            signature,
        }
    }

    /// Check the endorsement signature.
    pub fn verify(&self) -> Result<()> {
        verify_signature(
            &self.previous_key,
            &Self::message(&self.previous_key, &self.new_key, self.rotated_at),
            &self.signature,
        )
    }
}

/// A self-signed statement that a key must no longer be trusted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Revocation {
    /// Base64 public key being revoked.
    pub public_key: String,
    /// Unix timestamp (seconds) after which the key is invalid.
    pub revoked_at: u64,
    /// Human-readable reason (e.g. `"key compromised"`).
    pub reason: String,
    /// Base64 Ed25519 signature by the key itself over the revocation message.
    pub signature: String,
}

impl Revocation {
    fn message(public_key: &str, revoked_at: u64, reason: &str) -> Vec<u8> {
        format!("corrosive-revocation:{public_key}:{revoked_at}:{reason}").into_bytes()
    }

    /// Revoke `identity`'s key, effective now.
    pub fn create(identity: &AgentIdentity, reason: impl Into<String>) -> Self {
        let public_key = identity.public_key_base64();
        let revoked_at = unix_now();
        let reason = reason.into();
        let signature = identity.sign(&Self::message(&public_key, revoked_at, &reason));
        Self {
            public_key,
            revoked_at,
            reason,
            signature,
        }
    }

    /// Check the self-signature.
    pub fn verify(&self) -> Result<()> {
        verify_signature(
            &self.public_key,
            &Self::message(&self.public_key, self.revoked_at, &self.reason),
            &self.signature,
        )
    }
}

/// A consumer-side trust database: pinned keys plus known revocations.
///
/// Serializable, so it can be persisted and shared as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    /// Base64 public keys trusted as agent identities.
    trusted_keys: BTreeSet<String>,
    /// Known revocations, keyed by base64 public key.
    revocations: BTreeMap<String, Revocation>,
}

impl TrustStore {
    /// Create an empty trust store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin a key (base64) or a `did:key:` DID as trusted.
    pub fn trust(&mut self, key_or_did: &str) -> Result<()> {
        self.trusted_keys.insert(normalize_key(key_or_did)?);
        Ok(())
    }

    /// Record a revocation after checking its self-signature.
    pub fn revoke(&mut self, revocation: Revocation) -> Result<()> {
        revocation.verify()?;
        self.revocations
            .insert(revocation.public_key.clone(), revocation);
        Ok(())
    }

    /// Is this key (base64 or DID) directly trusted?
    pub fn is_trusted(&self, key_or_did: &str) -> bool {
        normalize_key(key_or_did)
            .map(|k| self.trusted_keys.contains(&k))
            .unwrap_or(false)
    }

    /// The revocation for a key, if one is known.
    pub fn revocation(&self, key_or_did: &str) -> Option<&Revocation> {
        normalize_key(key_or_did)
            .ok()
            .and_then(|k| self.revocations.get(&k))
    }

    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse from JSON (re-verifies every stored revocation).
    pub fn from_json(json: &str) -> Result<Self> {
        let store: Self = serde_json::from_str(json)?;
        for revocation in store.revocations.values() {
            revocation.verify()?;
        }
        Ok(store)
    }

    /// Verify a manifest against this trust store.
    ///
    /// Passes when **all** of the following hold:
    ///
    /// 1. the manifest signature is intact ([`AgentManifest::verify`]);
    /// 2. the current key is not revoked;
    /// 3. the current key is directly trusted, **or** the manifest's
    ///    `key_history` contains a valid, contiguous rotation chain from a
    ///    trusted key to the current key, where every rotation by a since-
    ///    revoked key happened *before* that key's revocation.
    pub fn verify_manifest(&self, manifest: &AgentManifest) -> Result<()> {
        manifest.verify()?;
        let current = manifest
            .public_key
            .as_deref()
            .ok_or_else(|| Error::Verification("manifest has no public key".into()))?;

        if let Some(revocation) = self.revocations.get(current) {
            return Err(Error::Verification(format!(
                "agent key is revoked ({})",
                revocation.reason
            )));
        }
        if self.trusted_keys.contains(current) {
            return Ok(());
        }

        // Not directly trusted: look for a rotation chain from a trusted key.
        let history = &manifest.key_history;
        if history.is_empty() {
            return Err(Error::Verification(
                "agent key is not trusted and manifest has no key history".into(),
            ));
        }
        let last = history.last().expect("history is non-empty");
        if last.new_key != current {
            return Err(Error::Verification(
                "key history does not end at the manifest's current key".into(),
            ));
        }

        let Some(start) = history
            .iter()
            .position(|proof| self.trusted_keys.contains(&proof.previous_key))
        else {
            return Err(Error::Verification(
                "no trusted key found in the rotation chain".into(),
            ));
        };

        for (i, proof) in history[start..].iter().enumerate() {
            proof.verify()?;
            // Chain must be contiguous: each rotation starts from the key the
            // previous rotation ended on.
            if let Some(next) = history[start..].get(i + 1) {
                if next.previous_key != proof.new_key {
                    return Err(Error::Verification(
                        "rotation chain is not contiguous".into(),
                    ));
                }
            }
            // A rotation signed by a revoked key only counts if it happened
            // before the revocation.
            if let Some(revocation) = self.revocations.get(&proof.previous_key) {
                if proof.rotated_at >= revocation.revoked_at {
                    return Err(Error::Verification(format!(
                        "rotation was made after key revocation ({})",
                        revocation.reason
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_manifest(identity: &AgentIdentity) -> AgentManifest {
        let mut manifest = AgentManifest::new("t", "1.0.0");
        manifest.sign(identity).unwrap();
        manifest
    }

    #[test]
    fn rotation_proof_roundtrip() {
        let old = AgentIdentity::generate();
        let new = AgentIdentity::generate();
        let proof = RotationProof::create(&old, &new);
        proof.verify().unwrap();

        let mut forged = proof.clone();
        forged.new_key = AgentIdentity::generate().public_key_base64();
        assert!(forged.verify().is_err());
    }

    #[test]
    fn directly_trusted_key_passes() {
        let identity = AgentIdentity::generate();
        let manifest = signed_manifest(&identity);
        let mut trust = TrustStore::new();
        trust.trust(&identity.public_key_base64()).unwrap();
        trust.verify_manifest(&manifest).unwrap();
    }

    #[test]
    fn untrusted_key_fails() {
        let manifest = signed_manifest(&AgentIdentity::generate());
        let trust = TrustStore::new();
        assert!(trust.verify_manifest(&manifest).is_err());
    }

    #[test]
    fn rotation_chain_extends_trust() {
        let k1 = AgentIdentity::generate();
        let k2 = AgentIdentity::generate();
        let k3 = AgentIdentity::generate();

        let mut manifest = signed_manifest(&k1);
        manifest.rotate_identity(&k1, &k2).unwrap();
        manifest.rotate_identity(&k2, &k3).unwrap();
        assert_eq!(manifest.public_key, Some(k3.public_key_base64()));

        let mut trust = TrustStore::new();
        trust.trust(&k1.public_key_base64()).unwrap();
        trust.verify_manifest(&manifest).unwrap();
    }

    #[test]
    fn revoked_current_key_fails() {
        let identity = AgentIdentity::generate();
        let manifest = signed_manifest(&identity);
        let mut trust = TrustStore::new();
        trust.trust(&identity.public_key_base64()).unwrap();
        trust
            .revoke(Revocation::create(&identity, "compromised"))
            .unwrap();
        let err = trust.verify_manifest(&manifest).unwrap_err();
        assert!(err.to_string().contains("revoked"));
    }

    #[test]
    fn rotation_before_revocation_still_valid() {
        let old = AgentIdentity::generate();
        let new = AgentIdentity::generate();

        let mut manifest = signed_manifest(&old);
        manifest.rotate_identity(&old, &new).unwrap();

        let mut trust = TrustStore::new();
        trust.trust(&old.public_key_base64()).unwrap();
        // Revoke the old key *after* the rotation (timestamps are seconds, so
        // craft a revocation strictly in the future of the proof).
        let mut revocation = Revocation::create(&old, "retired");
        revocation.revoked_at = manifest.key_history[0].rotated_at + 10;
        revocation.signature = old.sign(&Revocation::message(
            &revocation.public_key,
            revocation.revoked_at,
            &revocation.reason,
        ));
        trust.revoke(revocation).unwrap();

        trust.verify_manifest(&manifest).unwrap();
    }

    #[test]
    fn rotation_after_revocation_rejected() {
        let old = AgentIdentity::generate();
        let new = AgentIdentity::generate();

        let mut manifest = signed_manifest(&old);
        manifest.rotate_identity(&old, &new).unwrap();

        let mut trust = TrustStore::new();
        trust.trust(&old.public_key_base64()).unwrap();
        // Revocation timestamped before the rotation → the chain is invalid.
        let mut revocation = Revocation::create(&old, "compromised");
        revocation.revoked_at = manifest.key_history[0].rotated_at.saturating_sub(10);
        revocation.signature = old.sign(&Revocation::message(
            &revocation.public_key,
            revocation.revoked_at,
            &revocation.reason,
        ));
        trust.revoke(revocation).unwrap();

        assert!(trust.verify_manifest(&manifest).is_err());
    }

    #[test]
    fn trust_store_json_roundtrip() {
        let identity = AgentIdentity::generate();
        let mut trust = TrustStore::new();
        trust.trust(&identity.public_key_base64()).unwrap();
        trust
            .revoke(Revocation::create(&AgentIdentity::generate(), "test"))
            .unwrap();

        let restored = TrustStore::from_json(&trust.to_json().unwrap()).unwrap();
        assert!(restored.is_trusted(&identity.public_key_base64()));
    }
}
