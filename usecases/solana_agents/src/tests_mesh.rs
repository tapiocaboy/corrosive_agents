//! In-process end-to-end test of the decentralised mesh: an orchestrator, one
//! SVM specialist (pointed at a mock JSON-RPC server), and a client, all wired
//! over real libp2p on loopback. Exercises discovery, signed request/task/
//! result/response, the specialist's chain reads, and the orchestrator's
//! (LLM-less) fallback synthesis.

#![cfg(test)]

use std::time::Duration;

use crate::agent;
use crate::chains::ChainConfig;
use crate::client;
use crate::node;
use crate::orchestrator;
use crate::protocol::{ChainKind, ChainTag, Role};
use crate::test_support::spawn_mock_rpc;
use crate::worker;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mesh_runs_end_to_end_over_libp2p() {
    // No NVIDIA key → orchestrator uses fast, deterministic fallback synthesis.
    std::env::remove_var("NVIDIA_API_KEY");
    std::env::remove_var("NVIDIA_KEY");

    let rpc_url = spawn_mock_rpc().await;
    std::env::set_var("QUESTION", "What is the current slot of this network?");

    // Orchestrator on a fixed loopback port so the others can bootstrap to it.
    let seed_port = 49687u16;
    let seed = format!("/ip4/127.0.0.1/tcp/{seed_port}");

    let no_bootstrap: Vec<String> = Vec::new();
    let orch_agent = agent::build_agent(Role::Orchestrator, None).expect("orchestrator agent");
    let orch_node = node::start_with(&seed, &no_bootstrap).await.expect("orchestrator node");
    tokio::spawn(orchestrator::run(orch_node, orch_agent));

    // One SVM specialist, reading the mock server.
    let cfg = ChainConfig {
        tag: ChainTag { kind: ChainKind::Svm, name: "mock-svm".to_string() },
        endpoint: rpc_url,
    };
    let spec_agent = agent::build_agent(Role::Specialist, Some(&cfg)).expect("specialist agent");
    let spec_node = node::start_with("/ip4/0.0.0.0/tcp/0", std::slice::from_ref(&seed))
        .await
        .expect("specialist node");
    tokio::spawn(worker::run_worker(spec_node, spec_agent, cfg));

    // The client: discovers the orchestrator, asks, verifies the answer, returns.
    let client_agent = agent::build_agent(Role::Client, None).expect("client agent");
    let client_node = node::start_with("/ip4/0.0.0.0/tcp/0", &[seed])
        .await
        .expect("client node");

    let outcome = tokio::time::timeout(Duration::from_secs(45), client::run(client_node, client_agent)).await;

    let ran = outcome.expect("client timed out before receiving a verified answer");
    ran.expect("client returned an error");
}
