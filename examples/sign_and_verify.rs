//! Public-key verification of agents, end to end:
//! sign a manifest, verify it, detect tampering — no network or API key needed.
//!
//! ```sh
//! cargo run --example sign_and_verify
//! ```

use corrosive_agents::identity::verify_signature;
use corrosive_agents::prelude::*;

fn main() -> Result<()> {
    // 1. The publisher builds an agent with a (generated) identity.
    //    build() signs the manifest automatically.
    let agent = Agent::builder()
        .name("trusted-agent")
        .version("1.0.0")
        .description("An agent whose manifest can be verified by anyone")
        .capability(Capability::new("chat", "Conversational Q&A"))
        .generate_identity()
        .build()?;

    let manifest_json = agent.manifest().to_json()?;
    println!("signed manifest:\n{manifest_json}\n");

    // Persist the secret key to re-sign future releases (keep it safe!).
    let secret = agent
        .identity()
        .expect("identity was generated")
        .secret_key_base64();

    // 2. A consumer receives only the JSON manifest and verifies it —
    //    the public key travels inside the manifest.
    let received = AgentManifest::from_json(&manifest_json)?;
    received.verify()?;
    println!("verification with embedded key : OK");

    // 3. Stronger: pin the publisher's known public key.
    let publisher_key = agent.public_key().expect("agent has an identity");
    received.verify_with(&publisher_key)?;
    println!("verification with pinned key   : OK");

    // 4. Tampering is detected.
    let mut forged = received.clone();
    forged.description = "Totally harmless, trust me".into();
    match forged.verify() {
        Err(e) => println!("tampered manifest rejected     : {e}"),
        Ok(()) => unreachable!("tampering must not verify"),
    }

    // 5. The same identity can re-sign a new release.
    let identity = AgentIdentity::from_secret_base64(&secret)?;
    let mut next_release = AgentManifest::new("trusted-agent", "1.1.0");
    next_release.sign(&identity)?;
    next_release.verify_with(&publisher_key)?;
    println!("v1.1.0 signed with same key    : OK");

    // 6. Detached signatures work for arbitrary payloads too.
    let signature = identity.sign(b"any bytes at all");
    verify_signature(&publisher_key, b"any bytes at all", &signature)?;
    println!("detached signature             : OK");

    Ok(())
}
