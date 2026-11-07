//! WebSocket chat endpoint coverage (`/ws`): ping/pong, one-shot chat,
//! streaming chat, and the invalid-message error path.
#![cfg(feature = "server")]

mod common;

use std::sync::Arc;

use common::EchoLlm;
use corrosive_agents::prelude::*;
use corrosive_agents::server;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn ws_url() -> String {
    let agent = Arc::new(
        Agent::builder()
            .name("ws")
            .version("1.0.0")
            .llm(EchoLlm::default())
            .build()
            .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, server::router(agent)).await;
    });
    format!("ws://{addr}/ws")
}

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let msg = ws.next().await.expect("stream ended").expect("ws error");
        if msg.is_text() {
            return serde_json::from_str(msg.to_text().unwrap()).unwrap();
        }
    }
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(value.to_string())).await.unwrap();
}

#[tokio::test]
async fn ws_ping_chat_stream_and_error() {
    let url = ws_url().await;
    let (mut ws, _resp) = connect_async(url).await.unwrap();

    // ping → pong.
    send(&mut ws, json!({ "type": "ping" })).await;
    assert_eq!(next_json(&mut ws).await["type"], "pong");

    // chat (non-streaming) → reply.
    send(
        &mut ws,
        json!({ "type": "chat", "message": "hi", "session_id": "s" }),
    )
    .await;
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["type"], "reply");
    assert_eq!(reply["reply"], "echo: hi");
    assert_eq!(reply["session_id"], "s");

    // chat (streaming) → chunk(s) then done.
    send(
        &mut ws,
        json!({ "type": "chat", "message": "hi", "stream": true }),
    )
    .await;
    let mut text = String::new();
    loop {
        let m = next_json(&mut ws).await;
        match m["type"].as_str().unwrap() {
            "chunk" => text.push_str(m["delta"].as_str().unwrap()),
            "done" => break,
            other => panic!("unexpected message type: {other}"),
        }
    }
    assert_eq!(text, "hello");

    // Invalid JSON → error frame.
    ws.send(Message::Text("not json".into())).await.unwrap();
    assert_eq!(next_json(&mut ws).await["type"], "error");

    let _ = ws.close(None).await;
}
