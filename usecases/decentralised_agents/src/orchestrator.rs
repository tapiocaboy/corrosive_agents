//! The orchestrator: the mesh's front door.
//!
//! ```text
//!  client ──Request──▶ orchestrator
//!                          │  discovers peers over gossip, then:
//!                          ├─Route────▶ router      (plan)
//!                          ├─Answer───▶ qna         (factual answer)   ∥
//!                          ├─Reason───▶ reasoning   (analysis)
//!                          └─Synthesize▶ synthesizer (final answer)
//!  client ◀─Response── orchestrator
//! ```
//!
//! Every hop is a **signed** message over gossipsub; peers are discovered via
//! announcements and authenticated against the did:key each one advertises.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use corrosive_agents::prelude::*;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{sleep, timeout, Instant};
use uuid::Uuid;

use crate::node::{self, NodeHandle, Publisher};
use crate::protocol::{
    short, ClientRequest, ClientResponse, Envelope, Role, Stage, Task, TaskResult,
};

/// peer ids discovered for each role.
type Registry = Arc<Mutex<HashMap<Role, HashSet<String>>>>;
/// peer id → advertised did:key (pinned from announcements).
type Keys = Arc<Mutex<HashMap<String, String>>>;
/// in-flight worker tasks awaiting a result, keyed by correlation id.
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<TaskResult>>>>;

const REQUIRED: [Role; 4] = [Role::Router, Role::Qna, Role::Reasoning, Role::Synthesizer];

/// Entry point for the orchestrator node.
pub async fn run(mut handle: NodeHandle, agent: Agent) -> Result<()> {
    let publisher = handle.publisher();
    let identity = agent
        .identity()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("orchestrator needs an identity to sign messages"))?;
    let my_did = identity.did_key();

    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let keys: Keys = Arc::new(Mutex::new(HashMap::new()));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let me = publisher.peer_id.to_string();

    node::spawn_announcer(publisher.clone(), Role::Orchestrator, my_did.clone());
    tracing::info!(
        "📡 orchestrator online as [{}] — discovering specialists and awaiting client requests",
        short(&me)
    );

    // Inbound pump: keep discovery current, resolve results, queue requests.
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<ClientRequest>();
    {
        let registry = registry.clone();
        let keys = keys.clone();
        let pending = pending.clone();
        let me = me.clone();
        tokio::spawn(async move {
            while let Some(env) = handle.recv().await {
                match env {
                    Envelope::Announce(a) => {
                        let is_new = keys.lock().await.insert(a.peer.clone(), a.pubkey.clone()).is_none();
                        registry.lock().await.entry(a.role).or_default().insert(a.peer.clone());
                        if is_new {
                            tracing::info!(
                                "🔎 discovered [{}] role={} {}",
                                short(&a.peer),
                                a.role.as_str(),
                                a.pubkey
                            );
                        }
                    }
                    Envelope::Result(r) => {
                        if let Some(tx) = pending.lock().await.remove(&r.id) {
                            let _ = tx.send(r);
                        }
                    }
                    Envelope::Request(req) => {
                        if req.target == me {
                            let _ = req_tx.send(req);
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    let mesh = Mesh {
        publisher: publisher.clone(),
        me: me.clone(),
        identity,
        registry,
        keys,
        pending,
    };

    mesh.wait_for_peers().await?;
    tracing::info!("✅ mesh formed — all specialist roles discovered over gossip");

    // Serve client requests.
    while let Some(req) = req_rx.recv().await {
        if let Err(e) = mesh.serve_client(req).await {
            tracing::error!("request failed: {e:#}");
        }
        if env_flag("ORCHESTRATOR_ONESHOT") {
            // Let the swarm actually flush the response to the client before
            // we tear the process down (publish is fire-and-forget).
            sleep(Duration::from_secs(2)).await;
            tracing::info!("ORCHESTRATOR_ONESHOT set — exiting after one served request");
            break;
        }
    }
    Ok(())
}

/// Shared handle used by every pipeline step.
struct Mesh {
    publisher: Publisher,
    me: String,
    identity: AgentIdentity,
    registry: Registry,
    keys: Keys,
    pending: Pending,
}

impl Mesh {
    /// Block until every required role has announced at least one peer.
    async fn wait_for_peers(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let missing: Vec<&'static str> = {
                let reg = self.registry.lock().await;
                REQUIRED
                    .iter()
                    .filter(|r| reg.get(r).is_none_or(HashSet::is_empty))
                    .map(|r| r.as_str())
                    .collect()
            };
            if missing.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for peers; still missing: {missing:?}");
            }
            tracing::info!("⏳ waiting for specialists — missing: {missing:?}");
            sleep(Duration::from_secs(2)).await;
        }
    }

    /// Handle one client request end to end and reply with a signed answer.
    async fn serve_client(&self, req: ClientRequest) -> Result<()> {
        tracing::info!(
            "📥 client request {} from [{}]: {:?}",
            short(&req.id),
            short(&req.from),
            preview(&req.question, 80)
        );

        // Authenticate the client against its signature (and pinned key).
        match crate::protocol::verify(&req.pubkey, &req.id, &req.question, &req.sig) {
            Ok(()) => tracing::info!("🔏 verified client signature ✔ ({})", req.pubkey),
            Err(e) => tracing::warn!("⚠️  client signature INVALID: {e}"),
        }
        self.check_pinned(&req.from, &req.pubkey).await;

        let answer = self.pipeline(&req.question).await?;

        // Sign and return the final answer to the client.
        let (pubkey, sig) = crate::protocol::sign(&self.identity, &req.id, &answer);
        tracing::info!("📤 signing final answer → client [{}]", short(&req.from));
        self.publisher.publish(Envelope::Response(ClientResponse {
            id: req.id.clone(),
            target: req.from.clone(),
            from: self.me.clone(),
            answer,
            pubkey,
            sig,
        }));
        Ok(())
    }

    /// Run one query through Route → (Answer ∥ Reason) → Synthesize.
    async fn pipeline(&self, prompt: &str) -> Result<String> {
        pace().await;
        tracing::info!("➡️  stage 1/3 Route — asking the router to plan");
        let route = self.ask(Role::Router, Stage::Route, prompt, "").await?;
        tracing::info!("⬅️  router plan: {:?}", preview(&route.content, 90));

        pace().await;
        tracing::info!("➡️  stage 2/3 Answer ∥ Reason — fan out to qna + reasoning");
        let (qna, reasoning) = tokio::join!(
            self.ask(Role::Qna, Stage::Answer, prompt, &route.content),
            self.ask(Role::Reasoning, Stage::Reason, prompt, &route.content),
        );
        let qna = qna?;
        let reasoning = reasoning?;
        tracing::info!("⬅️  qna answer: {:?}", preview(&qna.content, 90));
        tracing::info!("⬅️  reasoning analysis: {:?}", preview(&reasoning.content, 90));

        pace().await;
        tracing::info!("➡️  stage 3/3 Synthesize — asking the synthesizer to merge");
        let context = format!(
            "Routing notes:\n{}\n\nFactual answer (QnA agent):\n{}\n\nReasoning analysis:\n{}",
            route.content, qna.content, reasoning.content
        );
        let synth = self
            .ask(Role::Synthesizer, Stage::Synthesize, prompt, &context)
            .await?;
        tracing::info!("⬅️  synthesizer returned the final answer ({} chars)", synth.content.len());
        Ok(synth.content.trim().to_string())
    }

    /// Pick a discovered peer for a role (first one; one per role here).
    async fn peer_for(&self, role: Role) -> Option<String> {
        self.registry
            .lock()
            .await
            .get(&role)
            .and_then(|s| s.iter().next().cloned())
    }

    /// Confirm a message's advertised key matches the one pinned at discovery.
    async fn check_pinned(&self, peer: &str, pubkey: &str) {
        if let Some(pinned) = self.keys.lock().await.get(peer) {
            if pinned == pubkey {
                tracing::info!("🔐 key matches the one pinned for [{}] at discovery", short(peer));
            } else {
                tracing::warn!("⚠️  key for [{}] does NOT match the pinned key!", short(peer));
            }
        }
    }

    /// Send a **signed** task to the current peer for `role` and await its
    /// result, verifying the worker's signature. Re-broadcasts every 3s until
    /// the result arrives or the deadline passes.
    async fn ask(
        &self,
        role: Role,
        stage: Stage,
        question: &str,
        context: &str,
    ) -> Result<TaskResult> {
        let target = loop {
            if let Some(peer) = self.peer_for(role).await {
                break peer;
            }
            tracing::info!("no {} peer yet — waiting", role.as_str());
            sleep(Duration::from_secs(1)).await;
        };

        let id = Uuid::new_v4().to_string();
        let (pubkey, sig) = crate::protocol::sign(&self.identity, &id, question);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);

        let task = Task {
            id: id.clone(),
            stage,
            origin: self.me.clone(),
            target: target.clone(),
            target_role: role,
            question: question.to_string(),
            context: context.to_string(),
            pubkey,
            sig,
        };
        tracing::info!(
            "📤 signed {:?} task {} → {} [{}]",
            stage,
            short(&id),
            role.as_str(),
            short(&target)
        );

        let resender = {
            let publisher = self.publisher.clone();
            let task = task.clone();
            tokio::spawn(async move {
                loop {
                    publisher.publish(Envelope::Task(task.clone()));
                    sleep(Duration::from_secs(3)).await;
                }
            })
        };

        let outcome = timeout(Duration::from_secs(180), rx).await;
        resender.abort();

        match outcome {
            Ok(Ok(result)) => {
                match crate::protocol::verify(&result.pubkey, &result.id, &result.content, &result.sig) {
                    Ok(()) => tracing::info!(
                        "🔏 verified {} result signature ✔",
                        result.from_role.as_str()
                    ),
                    Err(e) => tracing::warn!("⚠️  {} result signature INVALID: {e}", result.from_role.as_str()),
                }
                self.check_pinned(&result.from, &result.pubkey).await;
                Ok(result)
            }
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                bail!("result channel closed for {} task {id}", role.as_str())
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                bail!("timed out awaiting {} result for task {id}", role.as_str())
            }
        }
    }
}

/// Optional demo pacing: sleep `DEMO_PACE_MS` between pipeline stages so the
/// steps are readable in a recording. Off (0) by default.
async fn pace() {
    if let Ok(ms) = std::env::var("DEMO_PACE_MS") {
        if let Ok(ms) = ms.trim().parse::<u64>() {
            if ms > 0 {
                sleep(Duration::from_millis(ms)).await;
            }
        }
    }
}

/// Read a boolean-ish env var (`1`/`true`/`yes`/`on`, case-insensitive).
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// A short single-line preview of possibly-long, possibly-multiline text.
fn preview(s: &str, max: usize) -> String {
    let flat = s.trim().replace('\n', " ");
    if flat.chars().count() > max {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    } else {
        flat
    }
}
