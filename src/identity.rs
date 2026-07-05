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

    /// Sign arbitrary bytes, returning a base64-encoded Ed25519 signature.
    pub fn sign(&self, message: &[u8]) -> String {
        B64.encode(self.signing_key.sign(message).to_bytes())
    }
}

/// Verify a detached base64 Ed25519 `signature` over `message` with a
/// base64-encoded `public_key`.
///
/// Returns `Ok(())` when the signature is valid, [`Error::Verification`]
/// otherwise.
pub fn verify_signature(public_key: &str, message: &[u8], signature: &str) -> Result<()> {
    let key_bytes = B64
        .decode(public_key.trim())
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
}
