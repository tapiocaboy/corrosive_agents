//! X.509 certificate-based identity (feature `x509`).
//!
//! An agent's Ed25519 identity can be wrapped in a **self-signed X.509
//! certificate** — useful when the party you integrate with speaks PKI
//! rather than raw keys or DIDs. The certificate carries the same public key
//! that signs the agent's manifest, so verifying a manifest against a
//! certificate is: extract the key from the cert (checking the cert's own
//! self-signature), then verify the manifest with it.
//!
//! ```
//! use corrosive_agents::identity::AgentIdentity;
//! use corrosive_agents::agent::AgentManifest;
//! use corrosive_agents::x509;
//!
//! # fn main() -> corrosive_agents::Result<()> {
//! let identity = AgentIdentity::generate();
//! let cert_pem = x509::generate_certificate_pem(&identity, "research-agent")?;
//!
//! let mut manifest = AgentManifest::new("research-agent", "1.0.0");
//! manifest.sign(&identity)?;
//!
//! // A consumer holding only the certificate and the manifest:
//! x509::verify_manifest_with_certificate(&manifest, &cert_pem)?;
//! # Ok(())
//! # }
//! ```

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::agent::AgentManifest;
use crate::error::{Error, Result};
use crate::identity::AgentIdentity;

/// OID of the Ed25519 signature/key algorithm (RFC 8410).
const ED25519_OID: &str = "1.3.101.112";

/// Generate a self-signed X.509 certificate (PEM) for the identity, with
/// `common_name` as both subject CN and DNS SAN.
pub fn generate_certificate_pem(identity: &AgentIdentity, common_name: &str) -> Result<String> {
    let key_pair = rcgen::KeyPair::try_from(identity.pkcs8_der()?.as_slice())
        .map_err(|e| Error::Identity(format!("rcgen rejected the key: {e}")))?;
    let mut params = rcgen::CertificateParams::new(vec![common_name.to_string()])
        .map_err(|e| Error::Identity(format!("invalid certificate params: {e}")))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    let certificate = params
        .self_signed(&key_pair)
        .map_err(|e| Error::Identity(format!("certificate generation failed: {e}")))?;
    Ok(certificate.pem())
}

/// Extract the base64 Ed25519 public key from a PEM certificate, after
/// checking that the certificate is Ed25519-signed and its self-signature is
/// valid.
pub fn public_key_from_certificate_pem(cert_pem: &str) -> Result<String> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes())
        .map_err(|e| Error::Identity(format!("invalid PEM: {e}")))?;
    let certificate = pem
        .parse_x509()
        .map_err(|e| Error::Identity(format!("invalid X.509 certificate: {e}")))?;

    let spki = certificate.public_key();
    if spki.algorithm.algorithm.to_id_string() != ED25519_OID {
        return Err(Error::Identity(
            "certificate does not contain an Ed25519 key".into(),
        ));
    }
    let key_bytes: [u8; 32] = spki
        .subject_public_key
        .data
        .as_ref()
        .try_into()
        .map_err(|_| Error::Identity("certificate key must be 32 bytes".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| Error::Identity(format!("invalid Ed25519 key in certificate: {e}")))?;

    // Verify the certificate's self-signature over its TBS section.
    if certificate.signature_algorithm.algorithm.to_id_string() != ED25519_OID {
        return Err(Error::Identity(
            "certificate is not Ed25519-self-signed".into(),
        ));
    }
    let signature_bytes: [u8; 64] = certificate
        .signature_value
        .data
        .as_ref()
        .try_into()
        .map_err(|_| Error::Identity("certificate signature must be 64 bytes".into()))?;
    let signature = Signature::from_bytes(&signature_bytes);
    verifying_key
        .verify(certificate.tbs_certificate.as_ref(), &signature)
        .map_err(|_| Error::Verification("certificate self-signature is invalid".into()))?;

    Ok(B64.encode(key_bytes))
}

/// Verify a manifest against the identity in an X.509 certificate: the
/// certificate must be valid and self-consistent, and its key must have
/// signed the manifest.
pub fn verify_manifest_with_certificate(manifest: &AgentManifest, cert_pem: &str) -> Result<()> {
    let public_key = public_key_from_certificate_pem(cert_pem)?;
    manifest.verify_with(&public_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_roundtrip() {
        let identity = AgentIdentity::generate();
        let pem = generate_certificate_pem(&identity, "test-agent").unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"));

        let extracted = public_key_from_certificate_pem(&pem).unwrap();
        assert_eq!(extracted, identity.public_key_base64());
    }

    #[test]
    fn manifest_verifies_against_certificate() {
        let identity = AgentIdentity::generate();
        let pem = generate_certificate_pem(&identity, "signed-agent").unwrap();

        let mut manifest = AgentManifest::new("signed-agent", "1.0.0");
        manifest.sign(&identity).unwrap();
        verify_manifest_with_certificate(&manifest, &pem).unwrap();
    }

    #[test]
    fn foreign_certificate_is_rejected() {
        let identity = AgentIdentity::generate();
        let other = AgentIdentity::generate();
        let other_pem = generate_certificate_pem(&other, "imposter").unwrap();

        let mut manifest = AgentManifest::new("victim", "1.0.0");
        manifest.sign(&identity).unwrap();
        assert!(verify_manifest_with_certificate(&manifest, &other_pem).is_err());
    }

    #[test]
    fn garbage_pem_is_rejected() {
        assert!(public_key_from_certificate_pem("not a pem").is_err());
    }
}
