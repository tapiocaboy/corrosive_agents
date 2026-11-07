//! On-chain agents — `corrosive_agents` + NVIDIA Nemotron reading Solana-like
//! (SVM) and EVM chains, two ways:
//!
//! 1. **Solo** — one agent owns SVM + EVM skills and answers by LLM tool-calling:
//!
//!    ```sh
//!    cargo run -- solo "What's the current Solana slot and Ethereum block?"
//!    ```
//!
//! 2. **Mesh** — a decentralised libp2p mesh with one process per role. A
//!    `client` asks an `orchestrator`, which fans the question out to every
//!    discovered **chain specialist** over gossipsub and synthesises their
//!    signed on-chain facts. A specialist's role name *is* the network it serves:
//!
//!    ```sh
//!    ROLE=orchestrator     cargo run
//!    ROLE=solana-testnet   cargo run      # a specialist on Solana testnet
//!    ROLE=eclipse          cargo run      # another Solana-like (SVM) chain
//!    ROLE=client           cargo run
//!    ```
//!
//! Pick the role with the `ROLE` env var or the first CLI argument. Specialist
//! role names are chain presets (see `chains::PRESETS`) or a custom label with
//! `CHAIN_KIND` + `RPC_URL` set.

mod agent;
mod chains;
mod client;
mod node;
mod orchestrator;
mod protocol;
mod worker;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_mesh;

use anyhow::{Context, Result};
use protocol::Role;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env for local (non-Docker) runs; harmless if the file is absent.
    let _ = dotenvy::dotenv();
    init_tracing();

    let raw = raw_role()?;

    match resolve_launch(&raw)? {
        // The `solo` pseudo-role runs the single-process tool-calling demo.
        Launch::Solo => {
            let question = client::question();
            tracing::info!("▶️  single-process on-chain agent (LLM tool-calling over SVM + EVM)");
            let agent = agent::build_solo_agent().context("building the solo agent")?;
            agent::run_solo(agent, &question).await
        }
        // Control roles (client / orchestrator).
        Launch::Control(role) => {
            tracing::info!(role = role.as_str(), "starting mesh node");
            let agent = agent::build_agent(role, None).context("building the corrosive agent")?;
            let handle = node::start().await.context("starting the libp2p node")?;
            log_started(&handle);
            match role {
                Role::Client => client::run(handle, agent).await,
                Role::Orchestrator => orchestrator::run(handle, agent).await,
                Role::Specialist => unreachable!("resolve_launch maps specialists to Launch::Specialist"),
            }
        }
        // A chain specialist named by its network.
        Launch::Specialist(chain) => {
            tracing::info!(
                network = %chain.tag.name,
                kind = %chain.tag.kind.as_str(),
                endpoint = %chain.endpoint,
                "starting chain specialist"
            );
            let agent = agent::build_agent(Role::Specialist, Some(&chain))
                .context("building the corrosive agent")?;
            let handle = node::start().await.context("starting the libp2p node")?;
            log_started(&handle);
            worker::run_worker(handle, agent, chain).await
        }
    }
}

/// What a resolved `ROLE` asks the process to become.
enum Launch {
    /// Single-process tool-calling demo.
    Solo,
    /// A control role (client / orchestrator).
    Control(Role),
    /// A chain specialist on the given network.
    Specialist(chains::ChainConfig),
}

/// Map a raw `ROLE` string to a [`Launch`], resolving a chain specialist's
/// network (preset or custom). The pure routing core of `main`.
fn resolve_launch(raw: &str) -> Result<Launch> {
    if matches!(raw.trim().to_ascii_lowercase().as_str(), "solo" | "agent" | "demo") {
        return Ok(Launch::Solo);
    }
    if let Some(role) = Role::parse_control(raw) {
        return Ok(Launch::Control(role));
    }
    let chain =
        chains::resolve_chain(raw).with_context(|| format!("resolving chain for role '{raw}'"))?;
    Ok(Launch::Specialist(chain))
}

fn log_started(handle: &node::NodeHandle) {
    tracing::info!(
        peer = %protocol::short(&handle.peer_id.to_string()),
        "libp2p node started"
    );
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,solana_agents=info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn raw_role() -> Result<String> {
    std::env::var("ROLE")
        .ok()
        .or_else(|| std::env::args().nth(1))
        .context(
            "set ROLE (or pass it as the first argument): \
             solo | orchestrator | client | <chain> \
             (chain presets: solana-testnet, solana-devnet, solana-mainnet, eclipse, sonic-testnet, ethereum, base, polygon)",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ChainKind;

    #[test]
    fn resolve_launch_routes_each_role() {
        assert!(matches!(resolve_launch("solo").unwrap(), Launch::Solo));
        assert!(matches!(resolve_launch("DEMO").unwrap(), Launch::Solo));
        assert!(matches!(
            resolve_launch("orchestrator").unwrap(),
            Launch::Control(Role::Orchestrator)
        ));
        assert!(matches!(
            resolve_launch("client").unwrap(),
            Launch::Control(Role::Client)
        ));

        match resolve_launch("solana-testnet").unwrap() {
            Launch::Specialist(c) => {
                assert_eq!(c.tag.kind, ChainKind::Svm);
                assert_eq!(c.tag.name, "solana-testnet");
            }
            _ => panic!("expected a specialist launch"),
        }

        // An unknown name with no RPC_URL cannot be resolved.
        assert!(resolve_launch("totally-unknown-chain").is_err());
    }
}
