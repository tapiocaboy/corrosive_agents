//! Agent-to-agent (A2A) delegation with cryptographic peer verification —
//! fully offline (skills only, no API key required).
//!
//! One process plays both roles: a "worker" agent served over REST, and an
//! "orchestrator" that pins the worker's DID, verifies its signed manifest,
//! and delegates a skill to it.
//!
//! ```sh
//! cargo run --example a2a_delegation
//! ```

use std::sync::Arc;

use corrosive_agents::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    // ── The worker: a verifiable agent exposing a skill over REST ─────────
    let worker_identity = AgentIdentity::generate();
    let worker_did = worker_identity.did_key();

    let worker = Agent::builder()
        .name("worker")
        .version("1.0.0")
        .description("Crunches numbers on request")
        .skill(FnSkill::new(
            "fibonacci",
            "n-th Fibonacci number",
            |input| async move {
                let n = input["n"].as_u64().unwrap_or(0);
                let (mut a, mut b) = (0u64, 1u64);
                for _ in 0..n {
                    (a, b) = (b, a.saturating_add(b));
                }
                Ok(json!({ "n": n, "fib": a }))
            },
        ))
        .identity(worker_identity)
        .build()?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let router = corrosive_agents::server::router(Arc::new(worker));
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("worker server failed");
    });
    println!("worker serving at http://{addr}");
    println!("worker DID       : {worker_did}\n");

    // ── The orchestrator: pins the worker's DID before delegating ─────────
    let orchestrator = Agent::builder()
        .name("orchestrator")
        .version("1.0.0")
        .peer(
            "number-cruncher",
            RemoteAgent::new(format!("http://{addr}")).with_pinned_key(&worker_did),
        )
        .build()?;

    // First delegation triggers manifest fetch + signature + pinned-key check.
    for n in [10, 20, 42] {
        let out = orchestrator
            .delegate_skill("number-cruncher", "fibonacci", json!({ "n": n }))
            .await?;
        println!("fib({n:>2}) = {} (computed by verified peer)", out["fib"]);
    }

    // Delegation to an imposter identity fails before any work is sent.
    let imposter = AgentIdentity::generate();
    let bad_peer = RemoteAgent::new(format!("http://{addr}")).with_pinned_key(imposter.did_key());
    match bad_peer.execute_skill("fibonacci", json!({ "n": 1 })).await {
        Err(e) => println!("\nimposter pin rejected as expected: {e}"),
        Ok(_) => unreachable!("mismatched identity must not verify"),
    }

    Ok(())
}
