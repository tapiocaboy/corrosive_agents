//! REST server coverage: every route through a real axum server on an
//! ephemeral port, plus the API-key auth gate and graceful shutdown.
#![cfg(feature = "server")]

mod common;

use std::sync::Arc;

use common::EchoLlm;
use corrosive_agents::prelude::*;
use corrosive_agents::server;
use serde_json::{json, Value};

fn build_agent() -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .name("svc")
            .version("1.0.0")
            .description("served agent")
            .capability(Capability::new("chat", "conversation"))
            .llm(EchoLlm::default())
            .skill(FnSkill::new(
                "echo",
                "echoes input",
                |i| async move { Ok(i) },
            ))
            .generate_identity()
            .build()
            .unwrap(),
    )
}

async fn spawn(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn rest_endpoints_respond() {
    let agent = build_agent();
    let base = spawn(server::router(agent.clone())).await;
    let c = reqwest::Client::new();

    // Liveness.
    let r = c.get(format!("{base}/health")).send().await.unwrap();
    assert!(r.status().is_success());
    assert_eq!(r.json::<Value>().await.unwrap()["status"], "ok");

    // Readiness flips with set_ready.
    assert!(c
        .get(format!("{base}/ready"))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    agent.set_ready(false);
    assert_eq!(
        c.get(format!("{base}/ready"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );

    // Info / manifest / capabilities / skills.
    let info = c
        .get(format!("{base}/agent"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(info["name"], "svc");

    assert!(c
        .get(format!("{base}/agent/manifest"))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    let caps = c
        .get(format!("{base}/capabilities"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(caps[0]["name"], "chat");

    let skills = c
        .get(format!("{base}/skills"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(skills["skills"][0]["name"], "echo");

    // Execute a skill; unknown skill is a 404.
    let out = c
        .post(format!("{base}/skills/echo"))
        .json(&json!({ "x": 1 }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(out["x"], 1);
    assert_eq!(
        c.post(format!("{base}/skills/nope"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // Chat turn.
    let reply = c
        .post(format!("{base}/chat"))
        .json(&json!({ "message": "hi" }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(reply["reply"], "echo: hi");
    assert!(reply["session_id"].is_string());

    // Verify the agent's own (signed) manifest.
    let manifest = c
        .get(format!("{base}/agent/manifest"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let verdict = c
        .post(format!("{base}/verify"))
        .json(&manifest)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(verdict["valid"], true);
}

#[tokio::test]
async fn auth_gate_protects_non_probe_routes() {
    let agent = build_agent();
    let base = spawn(server::router_with_auth(
        agent,
        AuthScheme::api_keys(["secret"]),
    ))
    .await;
    let c = reqwest::Client::new();

    // Probes stay open.
    assert!(c
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    // Protected routes need the key.
    assert_eq!(
        c.get(format!("{base}/agent"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let ok = c
        .get(format!("{base}/agent"))
        .header("x-api-key", "secret")
        .send()
        .await
        .unwrap();
    assert!(ok.status().is_success());
}

#[tokio::test]
async fn serve_with_shutdown_binds_and_stops() {
    let agent = build_agent();

    // Discover a free port, then let the server bind it.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        server::serve_with_shutdown(agent, addr, async {
            let _ = rx.await;
        })
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let c = reqwest::Client::new();
    let r = c.get(format!("http://{addr}/health")).send().await.unwrap();
    assert!(r.status().is_success());

    let _ = tx.send(());
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    assert!(joined.is_ok(), "server did not shut down in time");
}

async fn free_addr() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

#[tokio::test]
async fn serve_and_serve_with_auth_bind() {
    let c = reqwest::Client::new();

    // The Arc::serve convenience method (plain, open).
    let addr = free_addr().await;
    let agent = build_agent();
    tokio::spawn(async move {
        let _ = agent.serve(addr).await;
    });

    // serve_with_auth.
    let addr2 = free_addr().await;
    let agent2 = build_agent();
    tokio::spawn(async move {
        let _ = server::serve_with_auth(agent2, addr2, AuthScheme::api_keys(["k"])).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert!(c
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    // Authenticated server rejects the unauthenticated request.
    assert_eq!(
        c.get(format!("http://{addr2}/agent"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}
