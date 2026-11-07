//! Agent construction.
//!
//! - [`build_agent`] — a mesh node's agent (fresh Ed25519 identity; a specialist
//!   additionally gets its chain's skills, the orchestrator an NVIDIA brain).
//! - [`build_solo_agent`] — one agent owning an SVM chain's + an EVM chain's
//!   skills, used by the single-process `solo` demo where the LLM picks tools.

use anyhow::{anyhow, Result};
use corrosive_agents::prelude::*;

use crate::chains::{self, ChainConfig};
use crate::protocol::Role;

/// Build the agent for a mesh role. A specialist must pass its resolved chain.
/// An LLM is attached only where a role needs one (the orchestrator) and only
/// when `NVIDIA_API_KEY` is set — the specialists read chains directly, so the
/// mesh's on-chain data works with no key.
pub fn build_agent(role: Role, chain: Option<&ChainConfig>) -> Result<Agent> {
    let llm = if role.wants_llm() { llm_from_env() } else { None };
    build_agent_with(role, chain, llm)
}

/// Resolve an NVIDIA client from the environment, logging (not failing) when no
/// key is set.
fn llm_from_env() -> Option<NvidiaClient> {
    match NvidiaClient::from_env() {
        Ok(mut client) => {
            if let Some(model) = chains::env_opt("NVIDIA_MODEL") {
                client = client.with_model(model);
            }
            Some(client)
        }
        Err(e) => {
            tracing::warn!(
                "no NVIDIA LLM ({e}); orchestrator will present the raw on-chain facts \
                 without LLM synthesis (set NVIDIA_API_KEY to enable it)"
            );
            None
        }
    }
}

/// Build an agent with an explicitly-supplied (optional) LLM — the testable core.
fn build_agent_with(role: Role, chain: Option<&ChainConfig>, llm: Option<NvidiaClient>) -> Result<Agent> {
    let mut builder = Agent::builder()
        .name(role.as_str())
        .version("0.1.0")
        .description(role.description())
        .generate_identity();

    // A specialist owns the skills for its chain.
    if role == Role::Specialist {
        let cfg = chain.ok_or_else(|| anyhow!("a specialist must be given a chain to serve"))?;
        for skill in chains::skills_for(cfg) {
            builder = builder.skill(skill);
        }
    }

    if let Some(client) = llm {
        builder = builder
            .system_prompt(role.system_prompt())
            .llm(client)
            .usage_observer(|e: &UsageEvent| {
                tracing::info!(
                    "🌐 NVIDIA NIM responded · served model={} · tokens prompt={} completion={} total={}",
                    e.model,
                    e.usage.prompt_tokens,
                    e.usage.completion_tokens,
                    e.usage.total_tokens
                );
            });
    }

    let agent = builder
        .build()
        .map_err(|e| anyhow!("failed to build {} agent: {e}", role.as_str()))?;

    if let Some(id) = agent.identity() {
        let what = match chain {
            Some(c) => format!("{} specialist", c.tag.name),
            None => role.as_str().to_string(),
        };
        tracing::info!("🔑 secure key creation — {what} identity (Ed25519) generated; {}", id.did_key());
    }
    Ok(agent)
}

/// Build the single-process demo agent: one agent that owns an SVM chain's and
/// an EVM chain's skills, and lets the model decide which tool to call.
pub fn build_solo_agent() -> Result<Agent> {
    let client = NvidiaClient::from_env().map_err(|e| {
        anyhow!("{e} (the solo demo needs NVIDIA_API_KEY so the model can pick tools)")
    })?;
    build_solo_agent_with(client)
}

/// Build the solo agent from an explicit client — the testable core.
fn build_solo_agent_with(client: NvidiaClient) -> Result<Agent> {
    let model =
        chains::env_opt("NVIDIA_MODEL").unwrap_or_else(|| models::NEMOTRON_35_LIGHTNING_30B.to_string());

    // One SVM endpoint + one EVM endpoint for the single agent (env-overridable).
    let svm_ep = chains::env_opt("SOLANA_RPC_URL")
        .unwrap_or_else(|| "https://api.devnet.solana.com".to_string());
    let evm_ep = chains::env_opt("EVM_RPC_URL")
        .unwrap_or_else(|| "https://ethereum-rpc.publicnode.com".to_string());

    let mut builder = Agent::builder()
        .name("onchain-agent")
        .version("0.1.0")
        .description("Answers questions by reading SVM and EVM chains via tools.")
        .system_prompt(SOLO_PROMPT)
        .llm(client.with_model(model))
        .usage_observer(|e: &UsageEvent| {
            tracing::info!(
                "🌐 NVIDIA tokens prompt={} completion={} total={} ({})",
                e.usage.prompt_tokens,
                e.usage.completion_tokens,
                e.usage.total_tokens,
                e.model
            );
        })
        .generate_identity();

    for skill in chains::svm_skills(&svm_ep, "solana") {
        builder = builder.skill(skill);
    }
    for skill in chains::evm_skills(&evm_ep, "ethereum") {
        builder = builder.skill(skill);
    }

    let agent = builder
        .build()
        .map_err(|e| anyhow!("failed to build the solo agent: {e}"))?;
    if let Some(id) = agent.identity() {
        tracing::info!("🔑 agent identity (Ed25519): {}", id.did_key());
    }
    Ok(agent)
}

/// Run the single-process tool-calling demo and print the answer.
pub async fn run_solo(agent: Agent, question: &str) -> Result<()> {
    tracing::info!("🙋 question: {question}");
    tracing::info!("🧠 asking the agent (LLM tool-calling over SVM + EVM skills)…");
    let answer = agent
        .chat_with_tools("solo", question, 6)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    print_answer(&answer);
    tracing::info!("token usage: {:?}", agent.usage());
    Ok(())
}

fn print_answer(answer: &str) {
    println!("\n\x1b[1;32m╔═══ ✅ ANSWER ════════════════════════════════════════════\x1b[0m");
    println!("\x1b[0;92m{}\x1b[0m", answer.trim());
    println!("\x1b[1;32m╚══════════════════════════════════════════════════════════\x1b[0m\n");
}

const SOLO_PROMPT: &str = "You are an on-chain assistant with tools to read Solana/SVM and Ethereum/EVM \
chains. When the user asks about balances, slots, genesis hashes, block heights, chain ids, or wants a \
devnet airdrop, CALL the appropriate tool and base your answer on the tool result — never guess a number. \
Solana/SVM account addresses are base58; EVM addresses start with 0x. Keep the answer concise and state \
the concrete values you fetched.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ChainKind, ChainTag};

    fn cfg(kind: ChainKind) -> ChainConfig {
        ChainConfig {
            tag: ChainTag { kind, name: "test-net".into() },
            endpoint: "http://127.0.0.1:1".into(),
        }
    }

    #[test]
    fn client_agent_has_identity_and_no_skills() {
        let agent = build_agent(Role::Client, None).unwrap();
        assert!(agent.identity().is_some());
        assert_eq!(agent.skills().len(), 0);
    }

    #[test]
    fn svm_specialist_registers_four_skills() {
        let agent = build_agent(Role::Specialist, Some(&cfg(ChainKind::Svm))).unwrap();
        assert!(agent.identity().is_some());
        assert_eq!(agent.skills().len(), 4);
    }

    #[test]
    fn evm_specialist_registers_three_skills() {
        let agent = build_agent(Role::Specialist, Some(&cfg(ChainKind::Evm))).unwrap();
        assert_eq!(agent.skills().len(), 3);
    }

    #[test]
    fn specialist_without_a_chain_is_an_error() {
        assert!(build_agent(Role::Specialist, None).is_err());
    }

    #[test]
    fn orchestrator_attaches_a_supplied_llm() {
        // An explicit client exercises the LLM-attach path with no env/network.
        let client = NvidiaClient::new("dummy-key-for-tests");
        let agent = build_agent_with(Role::Orchestrator, None, Some(client)).unwrap();
        assert!(agent.identity().is_some());
    }

    #[test]
    fn solo_agent_owns_both_chains_skills() {
        let client = NvidiaClient::new("dummy-key-for-tests");
        let agent = build_solo_agent_with(client).unwrap();
        assert!(agent.identity().is_some());
        // 4 SVM skills + 3 EVM skills.
        assert_eq!(agent.skills().len(), 7);
    }
}
