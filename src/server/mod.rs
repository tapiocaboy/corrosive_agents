//! REST + WebSocket serving (Tokio/axum), enabled with feature `server`
//! (on by default).
//!
//! # Endpoints
//!
//! | Method | Path              | Auth | Description                                |
//! |--------|-------------------|------|--------------------------------------------|
//! | GET    | `/health`         | no   | Liveness probe (always `ok` while serving) |
//! | GET    | `/ready`          | no   | Readiness probe ([`Agent::set_ready`])     |
//! | GET    | `/agent`          | yes  | [`AgentInfo`](crate::agent::AgentInfo)     |
//! | GET    | `/agent/manifest` | yes  | The (signed) JSON manifest                 |
//! | GET    | `/capabilities`   | yes  | Declared capabilities                      |
//! | GET    | `/skills`         | yes  | Registered skills with input schemas       |
//! | POST   | `/skills/{name}`  | yes  | Execute a skill (JSON body → JSON result)  |
//! | POST   | `/chat`           | yes  | One chat turn                              |
//! | POST   | `/verify`         | yes  | Verify a posted manifest's signature       |
//! | GET    | `/ws`             | yes  | WebSocket for interactive/streaming chat   |
//! | GET    | `/openapi.json`   | no   | OpenAPI 3 document (feature `openapi`)     |
//!
//! "Auth: yes" applies only when the router is built with
//! [`router_with_auth`] / [`serve_with_auth`]; the plain [`router`] is open.
//!
//! ```no_run
//! use std::sync::Arc;
//! use corrosive_agents::auth::AuthScheme;
//! use corrosive_agents::prelude::*;
//! use corrosive_agents::server;
//!
//! # async fn run() -> corrosive_agents::Result<()> {
//! let agent = Arc::new(Agent::builder().name("svc").version("0.1.0").build()?);
//!
//! // Open, until Ctrl-C/SIGTERM:
//! server::serve_with_shutdown(
//!     agent.clone(),
//!     "127.0.0.1:8080".parse().unwrap(),
//!     server::shutdown_signal(),
//! )
//! .await?;
//!
//! // Or API-key protected:
//! server::serve_with_auth(
//!     agent,
//!     "127.0.0.1:8080".parse().unwrap(),
//!     AuthScheme::api_keys(["super-secret"]),
//! )
//! .await?;
//! # Ok(())
//! # }
//! ```

mod rest;
mod ws;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;

use crate::agent::Agent;
use crate::auth::AuthScheme;
use crate::error::{Error, Result};

/// Build the axum [`Router`] for an agent (no authentication) — compose it
/// into a larger app or serve it directly with [`serve`].
pub fn router(agent: Arc<Agent>) -> Router {
    rest::router(agent, None)
}

/// Build the router with every endpoint except `/health`, `/ready`, and
/// `/openapi.json` protected by `auth`.
pub fn router_with_auth(agent: Arc<Agent>, auth: AuthScheme) -> Router {
    rest::router(agent, Some(Arc::new(auth)))
}

async fn bind(addr: SocketAddr) -> Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Server(format!("failed to bind {addr}: {e}")))
}

/// Bind `addr` and serve the agent's REST + WebSocket API until the task is
/// cancelled.
pub async fn serve(agent: Arc<Agent>, addr: SocketAddr) -> Result<()> {
    let listener = bind(addr).await?;
    tracing::info!("REST/WebSocket API listening on http://{addr}");
    axum::serve(listener, router(agent))
        .await
        .map_err(|e| Error::Server(e.to_string()))
}

/// [`serve`], with all non-probe endpoints protected by `auth`.
pub async fn serve_with_auth(agent: Arc<Agent>, addr: SocketAddr, auth: AuthScheme) -> Result<()> {
    let listener = bind(addr).await?;
    tracing::info!("REST/WebSocket API (authenticated) listening on http://{addr}");
    axum::serve(listener, router_with_auth(agent, auth))
        .await
        .map_err(|e| Error::Server(e.to_string()))
}

/// [`serve`], shutting down gracefully (in-flight requests drain) when
/// `signal` resolves. Pair with [`shutdown_signal`] for Ctrl-C/SIGTERM.
pub async fn serve_with_shutdown(
    agent: Arc<Agent>,
    addr: SocketAddr,
    signal: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = bind(addr).await?;
    tracing::info!("REST/WebSocket API listening on http://{addr} (graceful shutdown armed)");
    axum::serve(listener, router(agent))
        .with_graceful_shutdown(signal)
        .await
        .map_err(|e| Error::Server(e.to_string()))
}

/// Resolves on Ctrl-C or SIGTERM — the conventional shutdown trigger for
/// containerized deployments.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
    tracing::info!("shutdown signal received");
}

/// Serve REST + WebSocket over TLS (feature `tls`).
#[cfg(feature = "tls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls")))]
pub async fn serve_tls(
    agent: Arc<Agent>,
    addr: SocketAddr,
    tls: &crate::tls::TlsConfig,
) -> Result<()> {
    let (cert, key) = tls.pem_pair()?;
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(cert, key)
        .await
        .map_err(|e| Error::Server(format!("invalid TLS material: {e}")))?;
    tracing::info!("REST/WebSocket API listening on https://{addr}");
    axum_server::bind_rustls(addr, config)
        .serve(router(agent).into_make_service())
        .await
        .map_err(|e| Error::Server(e.to_string()))
}

impl Agent {
    /// Serve this agent's REST + WebSocket API on `addr`.
    ///
    /// Convenience for [`server::serve`](serve); requires the agent to be
    /// wrapped in an [`Arc`]. See also [`serve_with_auth`],
    /// [`serve_with_shutdown`], and [`serve_tls`].
    pub async fn serve(self: Arc<Self>, addr: SocketAddr) -> Result<()> {
        serve(self, addr).await
    }
}
