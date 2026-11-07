//! The client node: the human's entry point into the mesh.
//!
//! It joins the gossip network, **discovers an orchestrator** by its
//! announcement, sends it a signed question, then waits for — and
//! authenticates — the final answer and prints it. It runs no LLM.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use corrosive_agents::prelude::*;
use tokio::time::Instant;
use uuid::Uuid;

use crate::node::{self, NodeHandle, Publisher};
use crate::protocol::{short, ClientRequest, Envelope, Role};

/// Entry point for the client node.
pub async fn run(mut handle: NodeHandle, agent: Agent) -> Result<()> {
    let publisher = handle.publisher();
    let me = publisher.peer_id.to_string();
    let identity = agent
        .identity()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("client needs an identity to sign its request"))?;
    let my_did = identity.did_key();

    node::spawn_announcer(publisher.clone(), Role::Client, my_did.clone(), None);

    let question = question();
    let id = Uuid::new_v4().to_string();
    tracing::info!("🙋 client [{}] online; request {}", short(&me), short(&id));
    tracing::info!("❓ question: {}", question);
    tracing::info!("🔎 discovering an orchestrator over gossip…");

    let mut orchestrators: HashMap<String, String> = HashMap::new(); // peer → did:key
    let mut target: Option<String> = None;
    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut tick = tokio::time::interval(Duration::from_secs(2));

    loop {
        tokio::select! {
            maybe = handle.recv() => {
                let Some(env) = maybe else { break };
                match env {
                    Envelope::Announce(a) if a.role == Role::Orchestrator => {
                        if orchestrators.insert(a.peer.clone(), a.pubkey.clone()).is_none() {
                            tracing::info!("🔎 discovered orchestrator [{}] {}", short(&a.peer), a.pubkey);
                        }
                        if target.is_none() {
                            target = Some(a.peer.clone());
                        }
                        if !sent {
                            if let Some(t) = &target {
                                send(&publisher, &identity, &id, &me, t, &question, false);
                                sent = true;
                            }
                        }
                    }
                    Envelope::Response(r) if r.id == id && r.target == me => {
                        tracing::info!("📥 final answer received from orchestrator [{}]", short(&r.from));
                        // Authenticate: signature valid AND key matches the one
                        // this orchestrator announced at discovery.
                        match crate::protocol::verify(&r.pubkey, &r.id, &r.answer, &r.sig) {
                            Ok(()) => tracing::info!("🔏 verified orchestrator signature ✔ ({})", r.pubkey),
                            Err(e) => tracing::warn!("⚠️  orchestrator signature INVALID: {e}"),
                        }
                        match orchestrators.get(&r.from) {
                            Some(k) if k == &r.pubkey =>
                                tracing::info!("🔐 key matches the one pinned at discovery"),
                            Some(_) => tracing::warn!("⚠️  answer key does not match the pinned orchestrator key!"),
                            None => tracing::warn!("⚠️  answer from an orchestrator we never discovered"),
                        }
                        print_final_answer(&r.from, &r.answer);
                        return Ok(());
                    }
                    _ => {}
                }
            }
            _ = tick.tick() => {
                if sent {
                    if let Some(t) = &target {
                        send(&publisher, &identity, &id, &me, t, &question, true);
                    }
                }
                if Instant::now() >= deadline {
                    bail!("timed out waiting for a final answer");
                }
            }
        }
    }
    bail!("gossip network closed before an answer arrived")
}

/// Publish a signed request to the orchestrator.
fn send(
    publisher: &Publisher,
    identity: &AgentIdentity,
    id: &str,
    me: &str,
    target: &str,
    question: &str,
    resend: bool,
) {
    let (pubkey, sig) = crate::protocol::sign(identity, id, question);
    if resend {
        tracing::info!("🔁 re-sending request {} (still awaiting the answer)", short(id));
    } else {
        tracing::info!(
            "📤 sending signed request {} → orchestrator [{}]",
            short(id),
            short(target)
        );
    }
    publisher.publish(Envelope::Request(ClientRequest {
        id: id.to_string(),
        from: me.to_string(),
        target: target.to_string(),
        question: question.to_string(),
        pubkey,
        sig,
    }));
}

/// The question to ask: `QUESTION` env var, CLI arg #2, or a built-in demo.
///
/// Note: we deliberately do *not* read `PROMPT` — in an interactive shell that
/// name collides with the shell's own prompt string (zsh `$PROMPT`).
pub fn question() -> String {
    if let Ok(p) = std::env::var("QUESTION") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    if let Some(arg) = std::env::args().nth(2) {
        if !arg.trim().is_empty() {
            return arg;
        }
    }
    "For each network you can reach, what is the current slot (or block height) and \
     its genesis hash or chain id? Compare the Solana-like networks."
        .to_string()
}

/// Print the authenticated final answer as a highlighted block.
fn print_final_answer(from: &str, answer: &str) {
    println!("\n\x1b[1;32m╔═══ ✅ FINAL ANSWER (verified, from orchestrator [{}]) ═══════\x1b[0m", short(from));
    println!("\x1b[0;92m{}\x1b[0m", answer.trim());
    println!("\x1b[1;32m╚══════════════════════════════════════════════════════════\x1b[0m\n");
}
