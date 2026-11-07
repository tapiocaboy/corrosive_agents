//! Identity API coverage: key round-trips, did:key derivation, normalization,
//! and signature verification — all offline.

use corrosive_agents::identity::{self, AgentIdentity};

#[test]
fn identity_roundtrips_and_derivations() {
    let id = AgentIdentity::generate();

    // secret base64 round-trip.
    let secret_b64 = id.secret_key_base64();
    let restored = AgentIdentity::from_secret_base64(&secret_b64).unwrap();
    assert_eq!(restored.public_key_base64(), id.public_key_base64());
    assert_eq!(restored.did_key(), id.did_key());

    // Deterministic from raw secret bytes.
    let bytes = [7u8; 32];
    let a = AgentIdentity::from_secret_bytes(&bytes);
    let b = AgentIdentity::from_secret_bytes(&bytes);
    assert_eq!(a.public_key_base64(), b.public_key_base64());

    // did:key ⇄ public key.
    let pk = id.public_key_base64();
    let did = identity::did_key_from_public(&pk).unwrap();
    assert_eq!(did, id.did_key());
    assert_eq!(identity::public_key_from_did(&did).unwrap(), pk);

    // normalize accepts both a did:key and a raw base64 key.
    assert_eq!(identity::normalize_key(&did).unwrap(), pk);
    assert_eq!(identity::normalize_key(&pk).unwrap(), pk);

    // sign + verify.
    let sig = id.sign(b"hello");
    identity::verify_signature(&pk, b"hello", &sig).unwrap();
    assert!(identity::verify_signature(&pk, b"tampered", &sig).is_err());
}

#[test]
fn identity_rejects_bad_input() {
    assert!(AgentIdentity::from_secret_base64("not valid base64 !!!").is_err());
    assert!(identity::public_key_from_did("did:key:zNotRealMultibase").is_err());
    assert!(identity::normalize_key("***garbage***").is_err());
}
