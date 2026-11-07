//! Wire protocol spoken over the gossipsub topic.
//!
//! Every node publishes and subscribes to a single topic. Messages are
//! JSON-encoded [`Envelope`]s. gossipsub is a *broadcast* medium, so each
//! message carries explicit addressing (`target` peer id + `target_role`) and
//! every node simply ignores the messages that are not meant for it.

use corrosive_agents::prelude::AgentIdentity;
use serde::{Deserialize, Serialize};

/// The one gossipsub topic the whole mesh talks on.
pub const TOPIC: &str = "corrosive-agents/decentralised/v1";

/// The role a node plays in the mesh. Exactly one container per role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The end user's entry point: sends a request to the orchestrator and
    /// prints the final answer. Runs no LLM — it only holds an identity.
    Client,
    /// Coordinates the pipeline; the mesh's entry point and reasoner.
    Orchestrator,
    /// Classifies an incoming query and sketches a plan.
    Router,
    /// Answers questions factually and directly.
    Qna,
    /// Works a problem step by step and produces an analysis.
    Reasoning,
    /// Merges peer outputs into one final answer.
    Synthesizer,
}

impl Role {
    /// Stable lower-case identifier (matches the `ROLE` env value).
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Client => "client",
            Role::Orchestrator => "orchestrator",
            Role::Router => "router",
            Role::Qna => "qna",
            Role::Reasoning => "reasoning",
            Role::Synthesizer => "synthesizer",
        }
    }

    /// Does this role drive an LLM? (The client does not.)
    pub fn uses_llm(self) -> bool {
        !matches!(self, Role::Client)
    }

    /// Parse a role from the `ROLE` env var / CLI argument (lenient).
    pub fn parse(raw: &str) -> Option<Role> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "client" | "user" | "caller" => Some(Role::Client),
            "orchestrator" | "orchestra" | "coordinator" => Some(Role::Orchestrator),
            "router" | "route" => Some(Role::Router),
            "qna" | "q&a" | "qa" | "answer" => Some(Role::Qna),
            "reasoning" | "reason" | "reasoner" => Some(Role::Reasoning),
            "synthesizer" | "synthesiser" | "synth" | "synthesis" => Some(Role::Synthesizer),
            _ => None,
        }
    }

    /// One-line description used as the agent's manifest description.
    pub fn description(self) -> &'static str {
        match self {
            Role::Client => "Sends a request to the orchestrator and shows the final answer.",
            Role::Orchestrator => "Coordinates the decentralised agent mesh.",
            Role::Router => "Classifies queries and plans how to answer them.",
            Role::Qna => "Answers questions factually and concisely.",
            Role::Reasoning => "Reasons step by step about a problem.",
            Role::Synthesizer => "Synthesises peer outputs into a final answer.",
        }
    }

    /// The system prompt that gives the agent its behaviour.
    pub fn system_prompt(self) -> &'static str {
        match self {
            Role::Client => "", // the client holds an identity only; no LLM.
            Role::Orchestrator => {
                "You are the orchestrator of a cooperative agent mesh. You plan and \
                 sequence work across specialist peers."
            }
            Role::Router => {
                "You are a routing agent. Given a user query, respond with a short plan \
                 (under 70 words): restate the core question crisply, note whether it needs \
                 factual lookup, multi-step reasoning, or both, and list the key sub-points \
                 a good answer must cover. Do not answer the question yourself."
            }
            Role::Qna => {
                "You are a precise question-answering agent. Answer the user's question \
                 directly and factually in a few sentences. If you are unsure, say so. \
                 Do not pad the answer."
            }
            Role::Reasoning => {
                "You are a reasoning agent. Think through the problem step by step and \
                 produce a clear, well-structured analysis of the key considerations, \
                 trade-offs, and conclusions. Be rigorous but concise."
            }
            Role::Synthesizer => {
                "You are a synthesis agent. You are given a user question together with a \
                 factual answer and a reasoning analysis produced by peer agents. Merge them \
                 into one coherent, correct, and complete final answer for the user. Resolve \
                 any contradictions and keep it concise."
            }
        }
    }

    /// Build the user message handed to this role's LLM for a given task.
    pub fn compose_prompt(self, task: &Task) -> String {
        match self {
            Role::Router => format!("User query:\n{}", task.question),
            Role::Qna => format!("Question:\n{}", task.question),
            Role::Reasoning if task.context.trim().is_empty() => {
                format!("Problem:\n{}", task.question)
            }
            Role::Reasoning => format!(
                "Problem:\n{}\n\nRouting notes from the router agent:\n{}",
                task.question, task.context
            ),
            Role::Synthesizer => format!(
                "User question:\n{}\n\nMaterial gathered from peer agents:\n{}",
                task.question, task.context
            ),
            Role::Orchestrator | Role::Client => task.question.clone(),
        }
    }
}

/// Canonical bytes signed for a message carrying `id` and `body`. Binding the
/// correlation id into the signature stops a captured signature from being
/// replayed against a different request/result.
pub fn signing_message(id: &str, body: &str) -> String {
    format!("{id}\n{body}")
}

/// Sign `(id, body)` with an agent identity. Returns `(did:key, base64 sig)`.
pub fn sign(identity: &AgentIdentity, id: &str, body: &str) -> (String, String) {
    let msg = signing_message(id, body);
    (identity.did_key(), identity.sign(msg.as_bytes()))
}

/// Verify a `(pubkey, sig)` over `(id, body)`. `Ok(())` when authentic.
pub fn verify(pubkey: &str, id: &str, body: &str, sig: &str) -> corrosive_agents::Result<()> {
    let msg = signing_message(id, body);
    corrosive_agents::identity::verify_signature(pubkey, msg.as_bytes(), sig)
}

/// Which step of the pipeline a message belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Router classifies the query.
    Route,
    /// QnA answers the query.
    Answer,
    /// Reasoning analyses the query.
    Reason,
    /// Synthesizer merges the results.
    Synthesize,
}

/// A periodic presence/capability announcement (gossiped by every node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announce {
    /// The announcing node's libp2p peer id (as a string).
    pub peer: String,
    /// The role it plays.
    pub role: Role,
    /// Human-readable agent name.
    pub name: String,
    /// The node's agent identity as a W3C `did:key` (Ed25519). Peers pin this
    /// and verify every signed message the node sends against it.
    pub pubkey: String,
    /// Seconds since the Unix epoch — purely for logging/freshness.
    pub ts: u64,
}

/// A unit of work addressed from the orchestrator to one specialist peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Correlation id; the matching [`TaskResult`] carries the same value.
    pub id: String,
    /// Pipeline stage.
    pub stage: Stage,
    /// Peer id of the orchestrator (where the result should be sent back).
    pub origin: String,
    /// Peer id of the intended worker (chosen from the discovery registry).
    pub target: String,
    /// Role of the intended worker (defence in depth against mis-routing).
    pub target_role: Role,
    /// The user's question.
    pub question: String,
    /// Stage-specific context (routing notes, gathered answers, …).
    pub context: String,
    /// Orchestrator's did:key, so the worker can authenticate the task.
    pub pubkey: String,
    /// Ed25519 signature over `(id, question)` by the orchestrator.
    pub sig: String,
}

/// A worker's answer, addressed back to the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Correlation id (matches the originating [`Task::id`]).
    pub id: String,
    /// Pipeline stage this result is for.
    pub stage: Stage,
    /// Peer id of the orchestrator this result is addressed to.
    pub target: String,
    /// Peer id of the worker that produced it.
    pub from: String,
    /// Role of the worker that produced it.
    pub from_role: Role,
    /// The produced text.
    pub content: String,
    /// The worker's did:key, so the orchestrator can authenticate the result.
    pub pubkey: String,
    /// Ed25519 signature over `(id, content)` by the worker.
    pub sig: String,
}

/// A request from a client to the orchestrator (the mesh's front door).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRequest {
    /// Correlation id; the matching [`ClientResponse`] echoes it.
    pub id: String,
    /// Peer id of the client (where the answer is sent back).
    pub from: String,
    /// Peer id of the orchestrator the client discovered.
    pub target: String,
    /// The user's question.
    pub question: String,
    /// The client's did:key, so the orchestrator can authenticate the request.
    pub pubkey: String,
    /// Ed25519 signature over `(id, question)` by the client.
    pub sig: String,
}

/// The orchestrator's final answer, addressed back to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientResponse {
    /// Correlation id (matches [`ClientRequest::id`]).
    pub id: String,
    /// Peer id of the client this answer is for.
    pub target: String,
    /// Peer id of the orchestrator that assembled it.
    pub from: String,
    /// The synthesised final answer.
    pub answer: String,
    /// The orchestrator's did:key, so the client can authenticate the answer.
    pub pubkey: String,
    /// Ed25519 signature over `(id, answer)` by the orchestrator.
    pub sig: String,
}

/// Everything that crosses the wire is one of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Envelope {
    /// Presence/capability announcement.
    Announce(Announce),
    /// Client → orchestrator request.
    Request(ClientRequest),
    /// Orchestrator → client final answer.
    Response(ClientResponse),
    /// Work request (orchestrator → worker).
    Task(Task),
    /// Work result (worker → orchestrator).
    Result(TaskResult),
}

/// A short, log-friendly suffix of a peer id.
pub fn short(peer: &str) -> &str {
    if peer.len() > 8 {
        &peer[peer.len() - 8..]
    } else {
        peer
    }
}
