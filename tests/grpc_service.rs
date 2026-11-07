//! gRPC service coverage (feature `grpc`): all four RPCs round-tripped through
//! a real in-process tonic server + generated client, plus the auth interceptor.
#![cfg(feature = "grpc")]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::EchoLlm;
use corrosive_agents::grpc::pb;
use corrosive_agents::grpc::{self};
use corrosive_agents::prelude::*;
use futures_util::StreamExt;
use pb::agent_service_client::AgentServiceClient;

fn agent() -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .name("grpc-svc")
            .version("1.0.0")
            .description("grpc agent")
            .capability(Capability::new("chat", "conversation"))
            .llm(EchoLlm::default())
            .skill(FnSkill::new("echo", "echoes input", |i| async move { Ok(i) }))
            .generate_identity()
            .build()
            .unwrap(),
    )
}

async fn free_addr() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

async fn connect(addr: SocketAddr) -> AgentServiceClient<tonic::transport::Channel> {
    for _ in 0..40 {
        if let Ok(client) = AgentServiceClient::connect(format!("http://{addr}")).await {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("could not connect to the gRPC server");
}

#[tokio::test]
async fn all_rpcs_round_trip() {
    let addr = free_addr().await;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = grpc::serve_with_shutdown(agent(), addr, async {
            let _ = rx.await;
        })
        .await;
    });

    let mut client = connect(addr).await;

    // GetInfo.
    let info = client
        .get_info(pb::GetInfoRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.name, "grpc-svc");
    assert_eq!(info.capabilities[0].name, "chat");
    assert!(info.signed);
    assert!(info.skills.contains(&"echo".to_string()));

    // Chat (empty session id → server assigns one).
    let reply = client
        .chat(pb::ChatRequest {
            session_id: String::new(),
            message: "hi".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(reply.reply, "echo: hi");
    assert!(!reply.session_id.is_empty());

    // ExecuteSkill.
    let out = client
        .execute_skill(pb::SkillRequest {
            name: "echo".into(),
            input_json: "{\"x\":1}".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(out.output_json.contains("\"x\":1"));

    // ExecuteSkill on an unknown skill → NotFound status (to_status mapping).
    let status = client
        .execute_skill(pb::SkillRequest {
            name: "nope".into(),
            input_json: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);

    // ChatStream.
    let mut stream = client
        .chat_stream(pb::ChatRequest {
            session_id: String::new(),
            message: "hi".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut text = String::new();
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        if chunk.done {
            saw_done = true;
        } else {
            text.push_str(&chunk.delta);
        }
    }
    assert_eq!(text, "hello");
    assert!(saw_done);

    let _ = tx.send(());
}

#[tokio::test]
async fn auth_interceptor_rejects_then_allows() {
    let addr = free_addr().await;
    tokio::spawn(async move {
        let _ = grpc::serve_with_auth(agent(), addr, AuthScheme::api_keys(["secret"])).await;
    });

    let mut client = connect(addr).await;

    // No credentials → unauthenticated.
    let err = client.get_info(pb::GetInfoRequest {}).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // With the key in metadata → allowed.
    let mut req = tonic::Request::new(pb::GetInfoRequest {});
    req.metadata_mut()
        .insert("x-api-key", "secret".parse().unwrap());
    let info = client.get_info(req).await.unwrap().into_inner();
    assert_eq!(info.name, "grpc-svc");
}
