//! REST route handlers.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{Agent, AgentInfo, AgentManifest, Capability};
use crate::error::Error;

pub(crate) fn router(agent: Arc<Agent>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/agent", get(agent_info))
        .route("/agent/manifest", get(manifest))
        .route("/capabilities", get(capabilities))
        .route("/skills", get(list_skills))
        .route("/skills/{name}", post(execute_skill))
        .route("/chat", post(chat))
        .route("/verify", post(verify))
        .route("/ws", get(super::ws::upgrade))
        .with_state(agent)
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match &self {
            Error::SkillNotFound(_) => StatusCode::NOT_FOUND,
            Error::Config(_) | Error::Json(_) => StatusCode::BAD_REQUEST,
            Error::Verification(_) | Error::Identity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::NotConfigured(_) => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn agent_info(State(agent): State<Arc<Agent>>) -> Json<AgentInfo> {
    Json(agent.info())
}

async fn manifest(State(agent): State<Arc<Agent>>) -> Json<AgentManifest> {
    Json(agent.manifest().clone())
}

async fn capabilities(State(agent): State<Arc<Agent>>) -> Json<Vec<Capability>> {
    Json(agent.manifest().capabilities.clone())
}

async fn list_skills(State(agent): State<Arc<Agent>>) -> Json<Value> {
    let skills: Vec<Value> = agent
        .skills()
        .list()
        .iter()
        .map(|skill| {
            json!({
                "name": skill.name(),
                "description": skill.description(),
                "input_schema": skill.input_schema(),
            })
        })
        .collect();
    Json(json!({ "skills": skills }))
}

async fn execute_skill(
    State(agent): State<Arc<Agent>>,
    Path(name): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, Error> {
    Ok(Json(agent.execute_skill(&name, input).await?))
}

/// Body for `POST /chat`.
#[derive(Debug, Deserialize)]
struct ChatBody {
    /// Session to continue; a fresh one is created when omitted.
    session_id: Option<String>,
    /// The user message.
    message: String,
}

/// Response for `POST /chat`.
#[derive(Debug, Serialize)]
struct ChatReply {
    session_id: String,
    reply: String,
}

async fn chat(
    State(agent): State<Arc<Agent>>,
    Json(body): Json<ChatBody>,
) -> Result<Json<ChatReply>, Error> {
    let session_id = body
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let reply = agent.chat(&session_id, &body.message).await?;
    Ok(Json(ChatReply { session_id, reply }))
}

async fn verify(Json(manifest): Json<AgentManifest>) -> Json<Value> {
    match manifest.verify() {
        Ok(()) => Json(json!({ "valid": true })),
        Err(e) => Json(json!({ "valid": false, "reason": e.to_string() })),
    }
}
