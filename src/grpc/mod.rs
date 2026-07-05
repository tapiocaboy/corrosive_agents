//! gRPC serving (tonic), enabled with feature `grpc`.
//!
//! The protobuf-generated code is vendored in [`pb`] (see `proto/agent.proto`),
//! so **users of this crate do not need `protoc`**. The service exposes
//! `GetInfo`, `Chat`, `ChatStream` (server streaming), and `ExecuteSkill`.
//!
//! ```no_run
//! use std::sync::Arc;
//! use corrosive_agents::prelude::*;
//!
//! # async fn run() -> corrosive_agents::Result<()> {
//! let agent = Arc::new(Agent::builder().name("svc").version("0.1.0").build()?);
//! agent.serve_grpc("127.0.0.1:50051".parse().unwrap()).await?;
//! # Ok(())
//! # }
//! ```
//!
//! A client is generated too: [`pb::agent_service_client::AgentServiceClient`].

#[allow(missing_docs, unused_qualifications, clippy::all, clippy::pedantic)]
pub mod pb;

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use tonic::{Request, Response, Status};

use crate::agent::Agent;
use crate::error::{Error, Result};

use pb::agent_service_server::{AgentService, AgentServiceServer};

fn to_status(error: Error) -> Status {
    match &error {
        Error::SkillNotFound(_) => Status::not_found(error.to_string()),
        Error::Config(_) | Error::Json(_) => Status::invalid_argument(error.to_string()),
        Error::NotConfigured(_) => Status::unimplemented(error.to_string()),
        Error::Verification(_) | Error::Identity(_) => {
            Status::failed_precondition(error.to_string())
        }
        _ => Status::internal(error.to_string()),
    }
}

fn session_or_new(session_id: String) -> String {
    if session_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        session_id
    }
}

/// tonic service adapter exposing an [`Agent`] over gRPC.
pub struct AgentGrpcService {
    agent: Arc<Agent>,
}

impl AgentGrpcService {
    /// Wrap an agent for gRPC serving.
    pub fn new(agent: Arc<Agent>) -> Self {
        Self { agent }
    }

    /// The tonic server wrapper, ready to register with
    /// `tonic::transport::Server::builder().add_service(...)`.
    pub fn into_server(self) -> AgentServiceServer<Self> {
        AgentServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl AgentService for AgentGrpcService {
    async fn get_info(
        &self,
        _request: Request<pb::GetInfoRequest>,
    ) -> std::result::Result<Response<pb::AgentInfo>, Status> {
        let info = self.agent.info();
        Ok(Response::new(pb::AgentInfo {
            name: info.name,
            version: info.version,
            description: info.description,
            capabilities: info
                .capabilities
                .into_iter()
                .map(|c| pb::CapabilityInfo {
                    name: c.name,
                    description: c.description,
                    enabled: c.enabled,
                })
                .collect(),
            skills: info.skills,
            public_key: info.public_key.unwrap_or_default(),
            signed: info.signed,
        }))
    }

    async fn chat(
        &self,
        request: Request<pb::ChatRequest>,
    ) -> std::result::Result<Response<pb::ChatReply>, Status> {
        let message = request.into_inner();
        let session_id = session_or_new(message.session_id);
        let reply = self
            .agent
            .chat(&session_id, &message.message)
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::ChatReply { session_id, reply }))
    }

    type ChatStreamStream =
        Pin<Box<dyn Stream<Item = std::result::Result<pb::ChatChunk, Status>> + Send + 'static>>;

    async fn chat_stream(
        &self,
        request: Request<pb::ChatRequest>,
    ) -> std::result::Result<Response<Self::ChatStreamStream>, Status> {
        let message = request.into_inner();
        let session_id = session_or_new(message.session_id);
        let mut inner = self
            .agent
            .chat_stream(&session_id, &message.message)
            .await
            .map_err(to_status)?;

        let stream = async_stream::stream! {
            while let Some(chunk) = inner.next().await {
                match chunk {
                    Ok(chunk) => {
                        yield Ok(pb::ChatChunk {
                            session_id: session_id.clone(),
                            delta: chunk.delta,
                            done: chunk.done,
                        });
                    }
                    Err(e) => {
                        yield Err(to_status(e));
                        break;
                    }
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn execute_skill(
        &self,
        request: Request<pb::SkillRequest>,
    ) -> std::result::Result<Response<pb::SkillReply>, Status> {
        let message = request.into_inner();
        let input: serde_json::Value = if message.input_json.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&message.input_json).map_err(|e| {
                Status::invalid_argument(format!("input_json is not valid JSON: {e}"))
            })?
        };
        let output = self
            .agent
            .execute_skill(&message.name, input)
            .await
            .map_err(to_status)?;
        Ok(Response::new(pb::SkillReply {
            output_json: output.to_string(),
        }))
    }
}

/// Bind `addr` and serve the agent's gRPC API until the task is cancelled.
pub async fn serve(agent: Arc<Agent>, addr: SocketAddr) -> Result<()> {
    tracing::info!("gRPC API listening on {addr}");
    tonic::transport::Server::builder()
        .add_service(AgentGrpcService::new(agent).into_server())
        .serve(addr)
        .await
        .map_err(|e| Error::Server(format!("gRPC server failed: {e}")))
}

impl Agent {
    /// Serve this agent's gRPC API on `addr`.
    ///
    /// Convenience for [`grpc::serve`](serve); requires the agent to be
    /// wrapped in an [`Arc`].
    pub async fn serve_grpc(self: Arc<Self>, addr: SocketAddr) -> Result<()> {
        serve(self, addr).await
    }
}
