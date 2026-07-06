//! REST route handlers.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{Agent, AgentInfo, AgentManifest, Capability};
use crate::auth::AuthScheme;
use crate::error::Error;

pub(crate) fn router(agent: Arc<Agent>, auth: Option<Arc<AuthScheme>>) -> Router {
    // Probes (and the API spec) stay open so orchestrators can always reach
    // them; everything else is optionally auth-gated.
    let public = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready));
    #[cfg(feature = "openapi")]
    let public = public.route("/openapi.json", get(openapi));

    let mut protected = Router::new()
        .route("/agent", get(agent_info))
        .route("/agent/manifest", get(manifest))
        .route("/capabilities", get(capabilities))
        .route("/skills", get(list_skills))
        .route("/skills/{name}", post(execute_skill))
        .route("/chat", post(chat))
        .route("/verify", post(verify))
        .route("/ws", get(super::ws::upgrade));

    if let Some(auth) = auth {
        protected = protected.route_layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let auth = Arc::clone(&auth);
                async move { authenticate(&auth, request, next).await }
            },
        ));
    }

    public.merge(protected).with_state(agent)
}

async fn authenticate(auth: &AuthScheme, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let api_key = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    match auth.authorize(authorization, api_key) {
        Ok(()) => next.run(request).await,
        Err(e) => e.into_response(),
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match &self {
            Error::SkillNotFound(_) => StatusCode::NOT_FOUND,
            Error::Config(_) | Error::Json(_) => StatusCode::BAD_REQUEST,
            Error::Auth(_) => StatusCode::UNAUTHORIZED,
            Error::PermissionDenied(_) => StatusCode::FORBIDDEN,
            Error::Verification(_) | Error::Identity(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::NotConfigured(_) => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/health",
    responses((status = 200, description = "Liveness probe"))))]
async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/ready", responses(
    (status = 200, description = "Agent is ready for traffic"),
    (status = 503, description = "Agent is not ready"))))]
async fn ready(State(agent): State<Arc<Agent>>) -> Response {
    if agent.is_ready() {
        Json(json!({ "ready": true })).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ready": false })),
        )
            .into_response()
    }
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/agent",
    responses((status = 200, description = "Public agent info", body = AgentInfo))))]
async fn agent_info(State(agent): State<Arc<Agent>>) -> Json<AgentInfo> {
    Json(agent.info())
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/agent/manifest",
    responses((status = 200, description = "The signed agent manifest", body = AgentManifest))))]
async fn manifest(State(agent): State<Arc<Agent>>) -> Json<AgentManifest> {
    Json(agent.manifest().clone())
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/capabilities",
    responses((status = 200, description = "Declared capabilities", body = [Capability]))))]
async fn capabilities(State(agent): State<Arc<Agent>>) -> Json<Vec<Capability>> {
    Json(agent.manifest().capabilities.clone())
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/skills",
    responses((status = 200, description = "Registered skills with input schemas"))))]
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
                "required_permissions": skill.required_permissions(),
            })
        })
        .collect();
    Json(json!({ "skills": skills }))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/skills/{name}",
    params(("name" = String, Path, description = "Skill name")),
    responses(
        (status = 200, description = "Skill output (JSON)"),
        (status = 403, description = "Refused by the skill policy"),
        (status = 404, description = "No such skill"))))]
async fn execute_skill(
    State(agent): State<Arc<Agent>>,
    Path(name): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, Error> {
    Ok(Json(agent.execute_skill(&name, input).await?))
}

/// Body for `POST /chat`.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
struct ChatBody {
    /// Session to continue; a fresh one is created when omitted.
    session_id: Option<String>,
    /// The user message.
    message: String,
    /// When `true`, the model may call the agent's skills (tool loop).
    #[serde(default)]
    use_tools: bool,
}

/// Response for `POST /chat`.
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
struct ChatReply {
    session_id: String,
    reply: String,
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/chat",
    request_body = ChatBody,
    responses((status = 200, description = "Assistant reply", body = ChatReply))))]
async fn chat(
    State(agent): State<Arc<Agent>>,
    Json(body): Json<ChatBody>,
) -> Result<Json<ChatReply>, Error> {
    let session_id = body
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let reply = if body.use_tools {
        agent.chat_with_tools(&session_id, &body.message, 8).await?
    } else {
        agent.chat(&session_id, &body.message).await?
    };
    Ok(Json(ChatReply { session_id, reply }))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/verify",
    request_body = AgentManifest,
    responses((status = 200, description = "Verification verdict"))))]
async fn verify(Json(manifest): Json<AgentManifest>) -> Json<Value> {
    match manifest.verify() {
        Ok(()) => Json(json!({ "valid": true })),
        Err(e) => Json(json!({ "valid": false, "reason": e.to_string() })),
    }
}

#[cfg(feature = "openapi")]
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "corrosive_agents",
        description = "REST API of a corrosive agent (see also /ws for WebSocket chat)."
    ),
    paths(
        health,
        ready,
        agent_info,
        manifest,
        capabilities,
        list_skills,
        execute_skill,
        chat,
        verify
    ),
    components(schemas(AgentInfo, AgentManifest, Capability, ChatBody, ChatReply))
)]
struct ApiDoc;

#[cfg(feature = "openapi")]
async fn openapi() -> Json<Value> {
    use utoipa::OpenApi as _;
    Json(serde_json::to_value(ApiDoc::openapi()).unwrap_or(Value::Null))
}
