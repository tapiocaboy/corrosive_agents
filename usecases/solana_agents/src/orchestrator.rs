//! The orchestrator: the mesh's coordinator.
//!
//! It discovers chain specialists by their announcements — **any number, on any
//! networks** — accepts signed [`ClientRequest`]s, fans each question out to
//! every discovered specialist as a signed [`Task`], collects their signed
//! on-chain facts, synthesises a final answer with its LLM (or presents the raw
//! facts if it has none), and replies with a signed [`ClientResponse`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use corrosive_agents::prelude::*;

use crate::node::{self, NodeHandle, Publisher};
use crate::protocol::{short, ChainTag, ClientResponse, Envelope, Role, Task};

/// Let late specialists announce before concluding (one announce interval + margin).
const MIN_WAIT: Duration = Duration::from_secs(5);
/// Give up waiting for stragglers and synthesise with whatever arrived.
const JOB_TIMEOUT: Duration = Duration::from_secs(60);

/// An in-flight client request.
struct Job {
    client_peer: String,
    question: String,
    /// peer id → (which chain, its facts).
    facts: HashMap<String, (ChainTag, String)>,
    /// peer ids we have already sent this task to (for first-dispatch logging).
    dispatched: HashSet<String>,
    started: Instant,
}

/// Entry point for the orchestrator node.
pub async fn run(mut handle: NodeHandle, agent: Agent) -> Result<()> {
    let publisher = handle.publisher();
    let me = publisher.peer_id.to_string();
    let identity = agent
        .identity()
        .cloned()
        .ok_or_else(|| anyhow!("orchestrator needs an identity to sign tasks and answers"))?;
    let my_did = identity.did_key();

    node::spawn_announcer(publisher.clone(), Role::Orchestrator, my_did.clone(), None);
    tracing::info!(
        "🧭 orchestrator online as [{}] — discovering chain specialists & awaiting requests",
        short(&me)
    );

    // Discovered specialists: peer id → the chain it serves.
    let mut specialists: HashMap<String, ChainTag> = HashMap::new();
    // In-flight jobs, keyed by request id.
    let mut jobs: HashMap<String, Job> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));

    loop {
        tokio::select! {
            maybe = handle.recv() => {
                let Some(env) = maybe else { break };
                match env {
                    Envelope::Announce(a) => {
                        if a.role == Role::Specialist {
                            if let Some(tag) = a.chain {
                                if specialists.insert(a.peer.clone(), tag.clone()).is_none() {
                                    tracing::info!(
                                        "🔎 discovered {} specialist [{}] {}",
                                        tag.label(),
                                        short(&a.peer),
                                        a.pubkey
                                    );
                                }
                            }
                        }
                    }

                    Envelope::Request(req) => {
                        if req.target != me || jobs.contains_key(&req.id) {
                            continue; // not ours, or a resend we already track
                        }
                        match crate::protocol::verify(&req.pubkey, &req.id, &req.question, &req.sig) {
                            Ok(()) => tracing::info!("🔏 verified client signature ✔ ({})", req.pubkey),
                            Err(e) => tracing::warn!("⚠️  client signature INVALID: {e}"),
                        }
                        tracing::info!(
                            "📥 client request {} from [{}]: {}",
                            short(&req.id),
                            short(&req.from),
                            req.question
                        );
                        tracing::info!(
                            "📣 fanning out to {} discovered specialist(s)",
                            specialists.len()
                        );
                        jobs.insert(
                            req.id.clone(),
                            Job {
                                client_peer: req.from.clone(),
                                question: req.question.clone(),
                                facts: HashMap::new(),
                                dispatched: HashSet::new(),
                                started: Instant::now(),
                            },
                        );
                        if let Some(job) = jobs.get_mut(&req.id) {
                            dispatch(&publisher, &identity, &me, &specialists, &req.id, job);
                        }
                    }

                    Envelope::Result(res) => {
                        let complete = {
                            let Some(job) = jobs.get_mut(&res.id) else { continue };
                            match crate::protocol::verify(&res.pubkey, &res.id, &res.content, &res.sig) {
                                Ok(()) => tracing::info!(
                                    "🔏 verified {} specialist signature ✔",
                                    res.chain.label()
                                ),
                                Err(e) => tracing::warn!("⚠️  specialist signature INVALID: {e}"),
                            }
                            if job.facts.insert(res.from.clone(), (res.chain.clone(), res.content.clone())).is_none() {
                                tracing::info!(
                                    "📥 facts from {} specialist [{}] ({} chars) — {}/{} in",
                                    res.chain.label(),
                                    short(&res.from),
                                    res.content.len(),
                                    job.facts.len(),
                                    specialists.len()
                                );
                            }
                            is_complete(job, &specialists)
                        };
                        if complete {
                            if let Some(job) = jobs.remove(&res.id) {
                                finish_job(&publisher, &identity, &agent, &me, &res.id, job).await;
                            }
                        }
                    }

                    _ => {}
                }
            }

            _ = tick.tick() => {
                let ids: Vec<String> = jobs.keys().cloned().collect();
                for id in ids {
                    let timed_out = jobs.get(&id).map(|j| j.started.elapsed() > JOB_TIMEOUT).unwrap_or(false);
                    if timed_out {
                        tracing::warn!("⏳ job {} timed out; synthesising with partial facts", short(&id));
                        if let Some(job) = jobs.remove(&id) {
                            finish_job(&publisher, &identity, &agent, &me, &id, job).await;
                        }
                        continue;
                    }
                    // (Re)dispatch to specialists still missing, then check completion
                    // (specialists may have joined after the request arrived).
                    let complete = if let Some(job) = jobs.get_mut(&id) {
                        dispatch(&publisher, &identity, &me, &specialists, &id, job);
                        is_complete(job, &specialists)
                    } else {
                        false
                    };
                    if complete {
                        if let Some(job) = jobs.remove(&id) {
                            finish_job(&publisher, &identity, &agent, &me, &id, job).await;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// A job is done once at least one specialist is known, the settle window has
/// passed, and every discovered specialist has returned its facts.
fn is_complete(job: &Job, specialists: &HashMap<String, ChainTag>) -> bool {
    !specialists.is_empty()
        && job.started.elapsed() >= MIN_WAIT
        && specialists.keys().all(|peer| job.facts.contains_key(peer))
}

/// Send a signed task to each discovered specialist that has not yet returned
/// facts. First send to a peer logs at info; later resends at debug.
fn dispatch(
    publisher: &Publisher,
    identity: &AgentIdentity,
    me: &str,
    specialists: &HashMap<String, ChainTag>,
    id: &str,
    job: &mut Job,
) {
    for (peer, tag) in specialists {
        if job.facts.contains_key(peer) {
            continue;
        }
        let (pubkey, sig) = crate::protocol::sign(identity, id, &job.question);
        if job.dispatched.insert(peer.clone()) {
            tracing::info!("📤 dispatching task {} → {} [{}]", short(id), tag.name, short(peer));
        } else {
            tracing::debug!("🔁 re-sending task {} → {}", short(id), tag.name);
        }
        publisher.publish(Envelope::Task(Task {
            id: id.to_string(),
            origin: me.to_string(),
            target: peer.clone(),
            question: job.question.clone(),
            pubkey,
            sig,
        }));
    }
}

/// Synthesise the final answer and send it, signed, back to the client.
async fn finish_job(
    publisher: &Publisher,
    identity: &AgentIdentity,
    agent: &Agent,
    me: &str,
    id: &str,
    job: Job,
) {
    let answer = synthesize(agent, &job.question, &job.facts).await;
    tracing::info!(
        "🧩 final answer assembled from {} network(s) ({} chars) → replying to client [{}]",
        job.facts.len(),
        answer.len(),
        short(&job.client_peer)
    );
    let (pubkey, sig) = crate::protocol::sign(identity, id, &answer);
    publisher.publish(Envelope::Response(ClientResponse {
        id: id.to_string(),
        target: job.client_peer.clone(),
        from: me.to_string(),
        answer,
        pubkey,
        sig,
    }));
}

/// Merge the specialists' facts into one answer. Uses the orchestrator's LLM
/// when it has one; otherwise returns the facts verbatim.
async fn synthesize(agent: &Agent, question: &str, facts: &HashMap<String, (ChainTag, String)>) -> String {
    let mut material = String::new();
    for (tag, content) in facts.values() {
        material.push_str(&format!("\n## {} facts\n{}\n", tag.label(), content));
    }
    if material.is_empty() {
        material.push_str("\n(no specialists responded)\n");
    }

    let prompt = format!(
        "User question:\n{question}\n\n\
         Freshly-read on-chain facts from specialist peers (each labelled by network):{material}\n\n\
         Write the final answer for the user, attributing each figure to its network."
    );
    match agent.chat("orchestrator-synth", &prompt).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("synthesis LLM unavailable ({e}); returning the raw on-chain facts");
            format!("On-chain facts for: {question}\n{}", material.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::build_agent;
    use crate::protocol::ChainKind;

    fn tag(name: &str) -> ChainTag {
        ChainTag { kind: ChainKind::Svm, name: name.into() }
    }

    fn job_started_ago(secs: u64) -> Job {
        Job {
            client_peer: "client".into(),
            question: "q".into(),
            facts: HashMap::new(),
            dispatched: HashSet::new(),
            started: Instant::now()
                .checked_sub(Duration::from_secs(secs))
                .unwrap_or_else(Instant::now),
        }
    }

    #[test]
    fn is_complete_waits_for_all_specialists_and_the_settle_window() {
        let mut specialists = HashMap::new();
        specialists.insert("p1".to_string(), tag("solana-testnet"));

        // No facts yet → not complete even after the settle window.
        let mut job = job_started_ago(10);
        assert!(!is_complete(&job, &specialists));

        // All known specialists answered, window passed → complete.
        job.facts.insert("p1".to_string(), (tag("solana-testnet"), "slot: 1".into()));
        assert!(is_complete(&job, &specialists));

        // Same facts but within the settle window → not yet.
        let mut fresh = job_started_ago(0);
        fresh.facts.insert("p1".to_string(), (tag("solana-testnet"), "slot: 1".into()));
        assert!(!is_complete(&fresh, &specialists));

        // No specialists discovered at all → never "complete" (waits for timeout).
        let empty = job_started_ago(10);
        assert!(!is_complete(&empty, &HashMap::new()));
    }

    #[tokio::test]
    async fn synthesize_falls_back_to_raw_facts_without_an_llm() {
        // A client agent has no LLM, so `chat` errors and synthesis falls back.
        let agent = build_agent(Role::Client, None).unwrap();
        let mut facts = HashMap::new();
        facts.insert("p1".to_string(), (tag("solana-testnet"), "slot: 777".into()));

        let answer = synthesize(&agent, "what slot?", &facts).await;
        assert!(answer.contains("slot: 777"));
        assert!(answer.contains("solana-testnet"));
    }
}
