//! Agent construction and the worker event loop (Router / QnA / Reasoning /
//! Synthesizer).

use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use corrosive_agents::prelude::*;

use crate::node::{self, NodeHandle};
use crate::protocol::{short, Envelope, Role, TaskResult};

/// Build the `corrosive_agents::Agent` for a role.
///
/// Every role gets a freshly generated Ed25519 **agent identity** (its
/// `did:key`), which it uses to sign the messages it sends. Specialist roles
/// are additionally backed by a free NVIDIA NIM model; the `client` role holds
/// an identity only and never calls an LLM.
pub fn build_agent(role: Role) -> Result<Agent> {
    let mut builder = Agent::builder()
        .name(role.as_str())
        .version("0.1.0")
        .description(role.description())
        .generate_identity();

    if role.uses_llm() {
        let mut client = NvidiaClient::from_env().map_err(|e| {
            anyhow::anyhow!("{e} (set NVIDIA_API_KEY in your environment or .env)")
        })?;
        if let Some(model) = std::env::var("NVIDIA_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
        {
            client = client.with_model(model);
        }
        // Proof the calls are real: this fires after every completion with the
        // token accounting NVIDIA returned in its HTTP response.
        let usage_observer = |e: &UsageEvent| {
            tracing::info!(
                "🌐 NVIDIA NIM responded (integrate.api.nvidia.com) · served model={} · \
                 tokens prompt={} completion={} total={}",
                e.model,
                e.usage.prompt_tokens,
                e.usage.completion_tokens,
                e.usage.total_tokens
            );
        };
        builder = builder
            .system_prompt(role.system_prompt())
            .llm(client)
            .usage_observer(usage_observer);
    }

    let agent = builder
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build {} agent: {e}", role.as_str()))?;

    // Secure key creation: surface the agent's verifiable identity.
    if let Some(id) = agent.identity() {
        tracing::info!(
            "🔑 secure key creation — agent identity (Ed25519) generated; {}",
            id.did_key()
        );
    }
    if role.uses_llm() {
        let model = std::env::var("NVIDIA_MODEL")
            .unwrap_or_else(|_| "nvidia/nemotron-3.5-bolt-e4b".to_string());
        tracing::info!("🧠 brain: free NVIDIA NIM model {model}");
    }
    Ok(agent)
}

/// Run a specialist worker: announce presence, then answer any task addressed
/// to this node by invoking its LLM and gossiping a **signed** result back.
pub async fn run_worker(mut handle: NodeHandle, agent: Agent, role: Role) -> Result<()> {
    let publisher = handle.publisher();
    let me = publisher.peer_id.to_string();
    let identity = agent.identity().cloned();
    let my_did = identity.as_ref().map(|i| i.did_key()).unwrap_or_default();
    let model =
        std::env::var("NVIDIA_MODEL").unwrap_or_else(|_| "nvidia/nemotron-3.5-bolt-e4b".to_string());

    node::spawn_announcer(publisher.clone(), role, my_did.clone());
    tracing::info!(
        "📡 {} online as [{}] — announcing presence and awaiting tasks over gossipsub",
        role.as_str(),
        short(&me)
    );

    // Pinned did:keys learned from peer announcements (peer id → did:key).
    let mut known: HashMap<String, String> = HashMap::new();
    // Cache results per task id so a re-broadcast (sent by the orchestrator to
    // beat propagation races) never triggers a second, costly LLM call.
    let mut answered: HashMap<String, String> = HashMap::new();

    while let Some(env) = handle.recv().await {
        match env {
            Envelope::Announce(a) => {
                if known.insert(a.peer.clone(), a.pubkey.clone()).is_none() {
                    tracing::info!(
                        "🔎 discovered peer [{}] role={} {}",
                        short(&a.peer),
                        a.role.as_str(),
                        a.pubkey
                    );
                }
            }
            Envelope::Task(task) => {
                if task.target != me || task.target_role != role {
                    continue; // not addressed to us
                }
                tracing::info!(
                    "📥 received {:?} task {} from orchestrator [{}]",
                    task.stage,
                    short(&task.id),
                    short(&task.origin)
                );

                // Authenticate the task against the orchestrator's signature.
                match crate::protocol::verify(&task.pubkey, &task.id, &task.question, &task.sig) {
                    Ok(()) => tracing::info!("🔏 verified orchestrator signature ✔ ({})", task.pubkey),
                    Err(e) => tracing::warn!("⚠️  orchestrator signature INVALID: {e}"),
                }

                let was_cached = answered.contains_key(&task.id);
                let content = if let Some(cached) = answered.get(&task.id) {
                    tracing::info!("♻️  re-broadcast — returning cached answer (no second LLM call)");
                    cached.clone()
                } else {
                    let prompt = role.compose_prompt(&task);
                    tracing::info!(
                        "🧠 LIVE LLM call → POST https://integrate.api.nvidia.com/v1/chat/completions \
                         · model={model} · {} chars in …awaiting NVIDIA",
                        prompt.len()
                    );
                    let started = Instant::now();
                    let answer = match agent.chat(&task.id, prompt).await {
                        Ok(a) => {
                            tracing::info!(
                                "✅ NVIDIA reply ← {} chars in {} ms (real API round-trip)",
                                a.len(),
                                started.elapsed().as_millis()
                            );
                            a
                        }
                        Err(e) => {
                            tracing::warn!("LLM call failed: {e}");
                            format!("[{} could not answer: {e}]", role.as_str())
                        }
                    };
                    answered.insert(task.id.clone(), answer.clone());
                    answer
                };

                // The synthesizer is the end of the pipeline: show the final
                // answer right here, in its own pane.
                if role == Role::Synthesizer && !was_cached {
                    print_final_answer(&me, &content);
                }

                // Sign our result so the orchestrator can authenticate it.
                let (pubkey, sig) = match &identity {
                    Some(id) => crate::protocol::sign(id, &task.id, &content),
                    None => (String::new(), String::new()),
                };
                tracing::info!(
                    "📤 publishing signed result {} → orchestrator [{}]",
                    short(&task.id),
                    short(&task.origin)
                );
                publisher.publish(Envelope::Result(TaskResult {
                    id: task.id.clone(),
                    stage: task.stage,
                    target: task.origin.clone(),
                    from: me.clone(),
                    from_role: role,
                    content,
                    pubkey,
                    sig,
                }));
            }
            _ => {}
        }
    }

    Ok(())
}

/// Print the synthesised final answer as a highlighted block.
fn print_final_answer(me: &str, content: &str) {
    println!("\n\x1b[1;35m╔═══ 🧩 SYNTHESIZER [{}] · FINAL ANSWER ═══════════════════\x1b[0m", short(me));
    println!("\x1b[0;95m{}\x1b[0m", content.trim());
    println!("\x1b[1;35m╚══════════════════════════════════════════════════════════\x1b[0m\n");
}

