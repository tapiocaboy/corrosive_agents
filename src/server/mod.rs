//! REST + WebSocket serving (Tokio/axum), enabled with feature `server`
//! (on by default).
//!
//! # Endpoints
//!
//! | Method | Path              | Description                                |
//! |--------|-------------------|--------------------------------------------|
//! | GET    | `/health`         | Liveness probe                             |
//! | GET    | `/agent`          | [`AgentInfo`](crate::agent::AgentInfo)     |
//! | GET    | `/agent/manifest` | The (signed) JSON manifest                 |
//! | GET    | `/capabilities`   | Declared capabilities                      |
//! | GET    | `/skills`         | Registered skills with input schemas       |
//! | POST   | `/skills/{name}`  | Execute a skill (JSON body → JSON result)  |
//! | POST   | `/chat`           | One chat turn                              |
//! | POST   | `/verify`         | Verify a posted manifest's signature       |
//! | GET    | `/ws`             | WebSocket for interactive/streaming chat   |
//!
//! ```no_run
//! use std::sync::Arc;
//! use corrosive_agents::prelude::*;
//!
//! # async fn run() -> corrosive_agents::Result<()> {
//! let agent = Arc::new(Agent::builder().name("svc").version("0.1.0").build()?);
//! agent.serve("127.0.0.1:8080".parse().unwrap()).await?;
//! # Ok(())
//! # }
//! ```

mod rest;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;

use crate::agent::Agent;
use crate::error::{Error, Result};

/// Build the axum [`Router`] for an agent — compose it into a larger app or
/// serve it directly with [`serve`].
pub fn router(agent: Arc<Agent>) -> Router {
    rest::router(agent)
}

/// Bind `addr` and serve the agent's REST + WebSocket API until the task is
/// cancelled.
pub async fn serve(agent: Arc<Agent>, addr: SocketAddr) -> Result<()> {
    let app = router(agent);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Server(format!("failed to bind {addr}: {e}")))?;
    tracing::info!("REST/WebSocket API listening on http://{addr}");
    axum::serve(listener, app)
        .await
        .map_err(|e| Error::Server(e.to_string()))
}

impl Agent {
    /// Serve this agent's REST + WebSocket API on `addr`.
    ///
    /// Convenience for [`server::serve`](serve); requires the agent to be
    /// wrapped in an [`Arc`].
    pub async fn serve(self: Arc<Self>, addr: SocketAddr) -> Result<()> {
        serve(self, addr).await
    }
}
