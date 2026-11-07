//! Decentralised agent mesh — one binary, one role per process/container.
//!
//! Five cooperating `corrosive_agents` agents discover each other over libp2p
//! **mDNS** (local-IP discovery) and coordinate over **gossipsub**:
//!
//! - `orchestrator` — drives the pipeline and prints the final answer
//! - `router`       — classifies the query and plans the work
//! - `qna`          — answers factually
//! - `reasoning`    — reasons step by step
//! - `synthesizer`  — merges everything into a final answer
//!
//! Pick the role with the `ROLE` env var (or the first CLI argument):
//!
//! ```sh
//! ROLE=router cargo run
//! cargo run -- orchestrator
//! ```

mod client;
mod node;
mod orchestrator;
mod protocol;
mod roles;

use anyhow::{Context, Result};
use protocol::Role;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env for local (non-Docker) runs; harmless if the file is absent.
    let _ = dotenvy::dotenv();
    init_tracing();

    let role = resolve_role()?;
    tracing::info!(role = role.as_str(), "starting decentralised agent node");

    let agent = roles::build_agent(role).context("building the corrosive agent")?;
    let handle = node::start().await.context("starting the libp2p node")?;
    tracing::info!(peer = %protocol::short(&handle.peer_id.to_string()), "libp2p node started");

    match role {
        Role::Client => client::run(handle, agent).await,
        Role::Orchestrator => orchestrator::run(handle, agent).await,
        _ => roles::run_worker(handle, agent, role).await,
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,decentralised_agents=info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn resolve_role() -> Result<Role> {
    let raw = std::env::var("ROLE")
        .ok()
        .or_else(|| std::env::args().nth(1))
        .context(
            "set the ROLE env var (or pass it as the first argument): \
             orchestrator | router | qna | reasoning | synthesizer",
        )?;
    Role::parse(&raw).with_context(|| {
        format!("unknown role '{raw}' (expected orchestrator|router|qna|reasoning|synthesizer)")
    })
}
