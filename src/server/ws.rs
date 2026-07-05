//! WebSocket handler for interactive (optionally streaming) chat.
//!
//! # Protocol
//!
//! Client → server (JSON text frames):
//!
//! ```json
//! { "type": "chat", "message": "Hello!", "session_id": "abc", "stream": true }
//! { "type": "ping" }
//! ```
//!
//! Server → client:
//!
//! ```json
//! { "type": "reply", "session_id": "abc", "reply": "Hi!" }
//! { "type": "chunk", "session_id": "abc", "delta": "H" }
//! { "type": "done",  "session_id": "abc" }
//! { "type": "pong" }
//! { "type": "error", "message": "…" }
//! ```

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::agent::Agent;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Chat {
        message: String,
        session_id: Option<String>,
        #[serde(default)]
        stream: bool,
    },
    Ping,
}

pub(crate) async fn upgrade(State(agent): State<Arc<Agent>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle(socket, agent))
}

async fn handle(mut socket: WebSocket, agent: Arc<Agent>) {
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };

        let parsed: ClientMessage = match serde_json::from_str(text.as_str()) {
            Ok(parsed) => parsed,
            Err(e) => {
                let _ = send_json(
                    &mut socket,
                    json!({ "type": "error", "message": format!("invalid message: {e}") }),
                )
                .await;
                continue;
            }
        };

        match parsed {
            ClientMessage::Ping => {
                if send_json(&mut socket, json!({ "type": "pong" }))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            ClientMessage::Chat {
                message,
                session_id,
                stream,
            } => {
                let session_id = session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let outcome = if stream {
                    chat_streaming(&mut socket, &agent, &session_id, &message).await
                } else {
                    chat_once(&mut socket, &agent, &session_id, &message).await
                };
                if outcome.is_err() {
                    break; // socket gone
                }
            }
        }
    }
}

async fn chat_once(
    socket: &mut WebSocket,
    agent: &Agent,
    session_id: &str,
    message: &str,
) -> Result<(), axum::Error> {
    match agent.chat(session_id, message).await {
        Ok(reply) => {
            send_json(
                socket,
                json!({ "type": "reply", "session_id": session_id, "reply": reply }),
            )
            .await
        }
        Err(e) => send_json(socket, json!({ "type": "error", "message": e.to_string() })).await,
    }
}

async fn chat_streaming(
    socket: &mut WebSocket,
    agent: &Agent,
    session_id: &str,
    message: &str,
) -> Result<(), axum::Error> {
    let mut stream = match agent.chat_stream(session_id, message).await {
        Ok(stream) => stream,
        Err(e) => {
            return send_json(socket, json!({ "type": "error", "message": e.to_string() })).await;
        }
    };

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) if chunk.done => {
                return send_json(socket, json!({ "type": "done", "session_id": session_id }))
                    .await;
            }
            Ok(chunk) => {
                send_json(
                    socket,
                    json!({ "type": "chunk", "session_id": session_id, "delta": chunk.delta }),
                )
                .await?;
            }
            Err(e) => {
                return send_json(socket, json!({ "type": "error", "message": e.to_string() }))
                    .await;
            }
        }
    }
    send_json(socket, json!({ "type": "done", "session_id": session_id })).await
}

async fn send_json(socket: &mut WebSocket, value: serde_json::Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(value.to_string().into())).await
}
