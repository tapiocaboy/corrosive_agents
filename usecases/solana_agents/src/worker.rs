//! Chain specialist: announce the network it serves, then answer any task
//! addressed to this node by reading its chain **through its own registered
//! skills** and gossiping a signed, plain-text facts bundle back.

use std::collections::HashMap;

use anyhow::Result;
use corrosive_agents::prelude::*;
use serde_json::{json, Value};

use crate::chains::{self, ChainConfig};
use crate::node::{self, NodeHandle};
use crate::protocol::{short, ChainKind, Envelope, Role, TaskResult};

/// Run a chain specialist worker for the resolved `chain`.
pub async fn run_worker(mut handle: NodeHandle, agent: Agent, chain: ChainConfig) -> Result<()> {
    let publisher = handle.publisher();
    let me = publisher.peer_id.to_string();
    let identity = agent.identity().cloned();
    let my_did = identity.as_ref().map(|i| i.did_key()).unwrap_or_default();

    node::spawn_announcer(publisher.clone(), Role::Specialist, my_did.clone(), Some(chain.tag.clone()));
    tracing::info!(
        "📡 {} specialist online as [{}] — serving {} · awaiting tasks",
        chain.tag.name,
        short(&me),
        chain.endpoint
    );

    // Pinned did:keys learned from peer announcements (peer id → did:key).
    let mut known: HashMap<String, String> = HashMap::new();
    // Cache facts per task id so a re-broadcast (sent by the orchestrator to
    // beat propagation races) never triggers a second round of RPC calls.
    let mut answered: HashMap<String, String> = HashMap::new();

    while let Some(env) = handle.recv().await {
        match env {
            Envelope::Announce(a) => {
                if known.insert(a.peer.clone(), a.pubkey.clone()).is_none() {
                    let what = a.chain.as_ref().map(|c| c.label()).unwrap_or_else(|| a.role.as_str().to_string());
                    tracing::info!("🔎 discovered peer [{}] {} {}", short(&a.peer), what, a.pubkey);
                }
            }
            Envelope::Task(task) => {
                if task.target != me {
                    continue; // not addressed to us
                }
                tracing::info!(
                    "📥 received task {} from orchestrator [{}]",
                    short(&task.id),
                    short(&task.origin)
                );

                match crate::protocol::verify(&task.pubkey, &task.id, &task.question, &task.sig) {
                    Ok(()) => tracing::info!("🔏 verified orchestrator signature ✔ ({})", task.pubkey),
                    Err(e) => tracing::warn!("⚠️  orchestrator signature INVALID: {e}"),
                }

                let content = if let Some(cached) = answered.get(&task.id) {
                    tracing::info!("♻️  re-broadcast — returning cached facts (no second RPC round)");
                    cached.clone()
                } else {
                    tracing::info!("⛓️  reading {} over JSON-RPC…", chain.tag.name);
                    let facts = gather_facts(&agent, &chain, &task.question).await;
                    tracing::info!("✅ on-chain facts gathered ({} chars)", facts.len());
                    answered.insert(task.id.clone(), facts.clone());
                    facts
                };

                let (pubkey, sig) = match &identity {
                    Some(id) => crate::protocol::sign(id, &task.id, &content),
                    None => (String::new(), String::new()),
                };
                tracing::info!(
                    "📤 publishing signed {} facts {} → orchestrator [{}]",
                    chain.tag.name,
                    short(&task.id),
                    short(&task.origin)
                );
                publisher.publish(Envelope::Result(TaskResult {
                    id: task.id.clone(),
                    target: task.origin.clone(),
                    from: me.clone(),
                    chain: chain.tag.clone(),
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

/// Read the specialist's chain by invoking the agent's *registered skills*,
/// producing a plain-text facts bundle the orchestrator can synthesise.
async fn gather_facts(agent: &Agent, chain: &ChainConfig, question: &str) -> String {
    let skills = agent.skills();
    let mut lines: Vec<String> = vec![format!("network: {} · endpoint {}", chain.tag.name, chain.endpoint)];

    match chain.tag.kind {
        ChainKind::Svm => {
            match skills.execute("solana_get_slot", json!({})).await {
                Ok(v) => lines.push(format!("slot (chain height): {}", v["slot"])),
                Err(e) => lines.push(format!("getSlot failed: {e}")),
            }
            match skills.execute("solana_get_genesis_hash", json!({})).await {
                Ok(v) => lines.push(format!("genesis hash (network fingerprint): {}", as_str_or(&v["genesis_hash"], "?"))),
                Err(e) => lines.push(format!("getGenesisHash failed: {e}")),
            }
            if let Some(addr) = chains::find_solana_address(question) {
                match skills.execute("solana_get_balance", json!({ "address": addr })).await {
                    Ok(v) => lines.push(format!("balance of {addr}: {} SOL ({} lamports)", v["sol"], v["lamports"])),
                    Err(e) => lines.push(format!("getBalance({addr}) failed: {e}")),
                }
            }
        }
        ChainKind::Evm => {
            match skills.execute("evm_get_block_number", json!({})).await {
                Ok(v) => lines.push(format!("latest block: {}", v["block_number"])),
                Err(e) => lines.push(format!("eth_blockNumber failed: {e}")),
            }
            match skills.execute("evm_get_chain_id", json!({})).await {
                Ok(v) => lines.push(format!("chain id: {}", v["chain_id"])),
                Err(e) => lines.push(format!("eth_chainId failed: {e}")),
            }
            if let Some(addr) = chains::find_evm_address(question) {
                match skills.execute("evm_get_balance", json!({ "address": addr })).await {
                    Ok(v) => lines.push(format!("balance of {addr}: {} ETH ({} wei)", v["eth"], as_str_or(&v["wei"], "?"))),
                    Err(e) => lines.push(format!("eth_getBalance({addr}) failed: {e}")),
                }
            }
        }
    }

    lines.join("\n")
}

fn as_str_or<'a>(v: &'a Value, default: &'a str) -> &'a str {
    v.as_str().unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::build_agent;
    use crate::chains::ChainConfig;
    use crate::protocol::{ChainKind, ChainTag};

    #[tokio::test]
    async fn gather_facts_reads_svm_via_skills() {
        let url = crate::test_support::spawn_mock_rpc().await;
        let cfg = ChainConfig {
            tag: ChainTag { kind: ChainKind::Svm, name: "mock-svm".into() },
            endpoint: url,
        };
        let agent = build_agent(Role::Specialist, Some(&cfg)).unwrap();
        let facts =
            gather_facts(&agent, &cfg, "balance of 9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM?").await;
        assert!(facts.contains("mock-svm"));
        assert!(facts.contains("123456")); // slot
        assert!(facts.contains("genesis"));
        assert!(facts.contains("SOL")); // balance line (address was present)
    }

    #[tokio::test]
    async fn gather_facts_reads_evm_via_skills() {
        let url = crate::test_support::spawn_mock_rpc().await;
        let cfg = ChainConfig {
            tag: ChainTag { kind: ChainKind::Evm, name: "mock-evm".into() },
            endpoint: url,
        };
        let agent = build_agent(Role::Specialist, Some(&cfg)).unwrap();
        let facts =
            gather_facts(&agent, &cfg, "balance of 0x742d35Cc6634C0532925a3b844Bc454e4438f44e?").await;
        assert!(facts.contains("latest block"));
        assert!(facts.contains("chain id"));
        assert!(facts.contains("ETH")); // balance line (0x address present)
    }
}
