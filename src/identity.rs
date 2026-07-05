//! Agent identity: Ed25519 keypairs for signing and verifying agents.
//!
//! Every agent may carry an [`AgentIdentity`]. Signing an
//! [`AgentManifest`](crate::agent::AgentManifest) embeds the public key and a
//! signature over the manifest's canonical JSON, so any third party holding
//! nothing but the manifest can check that it is authentic and untampered.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::error::{Error, Result};

/// An Ed25519 keypair that gives an agent a verifiable identity.
#[derive(Clone)]
pub struct AgentIdentity {
    signing_key: SigningKey,
}

impl std::fmt::Debug for AgentIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never leak the secret key through Debug output.
        f.debug_struct("AgentIdentity")
            .field("public_key", &self.public_key_base64())
            .finish_non_exhaustive()
    }
}

impl AgentIdentity {
    /// Generate a fresh random keypair using the OS entropy source.
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut rand::rngs::OsRng),
        }
    }

    /// Restore an identity from the 32-byte secret key.
    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(bytes),
        }
    }

    /// Restore an identity from a base64-encoded secret key
    /// (as produced by [`AgentIdentity::secret_key_base64`]).
    pub fn from_secret_base64(encoded: &str) -> Result<Self> {
        let bytes = B64
            .decode(encoded.trim())
            .map_err(|e| Error::Identity(format!("invalid base64 secret key: {e}")))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Identity("secret key must be exactly 32 bytes".into()))?;
        Ok(Self::from_secret_bytes(&bytes))
    }

    /// The base64-encoded secret key. **Store securely, never publish.**
    pub fn secret_key_base64(&self) -> String {
        B64.encode(self.signing_key.to_bytes())
    }

    /// The base64-encoded public key — safe to share and embed in manifests.
    pub fn public_key_base64(&self) -> String {
        B64.encode(self.signing_key.verifying_key().to_bytes())
    }

    /// The identity as a W3C `did:key` DID (Ed25519 multicodec,
    /// base58btc multibase), e.g. `did:key:z6Mk…`.
    pub fn did_key(&self) -> String {
        did_key_from_public_bytes(&self.signing_key.verifying_key().to_bytes())
    }

    /// Sign arbitrary bytes, returning a base64-encoded Ed25519 signature.
    pub fn sign(&self, message: &[u8]) -> String {
        B64.encode(self.signing_key.sign(message).to_bytes())
    }
}

/// Multicodec prefix identifying an Ed25519 public key inside a `did:key`.
const DID_KEY_ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];

fn did_key_from_public_bytes(public_key: &[u8; 32]) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&DID_KEY_ED25519_MULTICODEC);
    bytes.extend_from_slice(public_key);
    format!("did:key:z{}", bs58::encode(bytes).into_string())
}

/// Convert a base64 Ed25519 public key into its `did:key` form.
pub fn did_key_from_public(public_key_base64: &str) -> Result<String> {
    let bytes = B64
        .decode(public_key_base64.trim())
        .map_err(|e| Error::Identity(format!("invalid base64 public key: {e}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Identity("public key must be exactly 32 bytes".into()))?;
    Ok(did_key_from_public_bytes(&bytes))
}

/// Extract the base64 Ed25519 public key from a `did:key:z…` DID.
pub fn public_key_from_did(did: &str) -> Result<String> {
    let encoded = did.trim().strip_prefix("did:key:z").ok_or_else(|| {
        Error::Identity("only did:key with base58btc multibase ('z') is supported".into())
    })?;
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| Error::Identity(format!("invalid base58 in did:key: {e}")))?;
    let key = bytes
        .strip_prefix(&DID_KEY_ED25519_MULTICODEC)
        .ok_or_else(|| Error::Identity("did:key does not contain an Ed25519 key".into()))?;
    if key.len() != 32 {
        return Err(Error::Identity(
            "did:key payload must be a 32-byte Ed25519 key".into(),
        ));
    }
    Ok(B64.encode(key))
}

/// Accept a key in either form — base64 or `did:key:` — and return it
/// normalized to base64. Everything in this crate that verifies keys
/// (manifests, trust stores, pinned A2A peers) accepts both.
pub fn normalize_key(key_or_did: &str) -> Result<String> {
    let trimmed = key_or_did.trim();
    if trimmed.starts_with("did:key:") {
        return public_key_from_did(trimmed);
    }
    // Validate that it actually is a 32-byte base64 key.
    let bytes = B64
        .decode(trimmed)
        .map_err(|e| Error::Identity(format!("invalid base64 public key: {e}")))?;
    if bytes.len() != 32 {
        return Err(Error::Identity(
            "public key must be exactly 32 bytes".into(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Verify a detached base64 Ed25519 `signature` over `message` with a
/// `public_key` given as base64 or as a `did:key:` DID.
///
/// Returns `Ok(())` when the signature is valid, [`Error::Verification`]
/// otherwise.
pub fn verify_signature(public_key: &str, message: &[u8], signature: &str) -> Result<()> {
    let key_bytes = B64
        .decode(normalize_key(public_key)?)
        .map_err(|e| Error::Identity(format!("invalid base64 public key: {e}")))?;
    let key_bytes: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| Error::Identity("public key must be exactly 32 bytes".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| Error::Identity(format!("invalid Ed25519 public key: {e}")))?;

    let sig_bytes = B64
        .decode(signature.trim())
        .map_err(|e| Error::Identity(format!("invalid base64 signature: {e}")))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| Error::Identity("signature must be exactly 64 bytes".into()))?;
    let signature = Signature::from_bytes(&sig_bytes);

    verifying_key
        .verify(message, &signature)
        .map_err(|_| Error::Verification("Ed25519 signature does not match".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let identity = AgentIdentity::generate();
        let sig = identity.sign(b"hello agent");
        verify_signature(&identity.public_key_base64(), b"hello agent", &sig).unwrap();
    }

    #[test]
    fn tampered_message_fails() {
        let identity = AgentIdentity::generate();
        let sig = identity.sign(b"hello agent");
        let err =
            verify_signature(&identity.public_key_base64(), b"hello agent!", &sig).unwrap_err();
        assert!(matches!(err, Error::Verification(_)));
    }

    #[test]
    fn secret_key_roundtrip() {
        let identity = AgentIdentity::generate();
        let restored = AgentIdentity::from_secret_base64(&identity.secret_key_base64()).unwrap();
        assert_eq!(identity.public_key_base64(), restored.public_key_base64());
    }

    #[test]
    fn wrong_key_fails() {
        let identity = AgentIdentity::generate();
        let other = AgentIdentity::generate();
        let sig = identity.sign(b"msg");
        assert!(verify_signature(&other.public_key_base64(), b"msg", &sig).is_err());
    }

    #[test]
    fn did_key_roundtrip() {
        let identity = AgentIdentity::generate();
        let did = identity.did_key();
        assert!(
            did.starts_with("did:key:z6Mk"),
            "ed25519 did:key starts with z6Mk, got {did}"
        );
        assert_eq!(
            public_key_from_did(&did).unwrap(),
            identity.public_key_base64()
        );
        assert_eq!(
            did_key_from_public(&identity.public_key_base64()).unwrap(),
            did
        );
    }

    #[test]
    fn verify_accepts_did_as_key() {
        let identity = AgentIdentity::generate();
        let sig = identity.sign(b"did message");
        verify_signature(&identity.did_key(), b"did message", &sig).unwrap();
    }

    #[test]
    fn normalize_key_accepts_both_forms() {
        let identity = AgentIdentity::generate();
        let b64 = identity.public_key_base64();
        assert_eq!(normalize_key(&b64).unwrap(), b64);
        assert_eq!(normalize_key(&identity.did_key()).unwrap(), b64);
        assert!(normalize_key("did:key:not-multibase").is_err());
        assert!(normalize_key("not base64!!!").is_err());
    }
}
