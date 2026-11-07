//! Wire protocol spoken over the gossipsub topic.
//!
//! Every node publishes and subscribes to one topic. Messages are
//! JSON-encoded [`Envelope`]s. gossipsub is a *broadcast* medium, so each
//! message carries explicit addressing (the `target` peer id) and every node
//! ignores the messages that are not meant for it. Requests, tasks and results
//! are Ed25519-signed by the sending agent's identity, and receivers pin the
//! `did:key` learned at discovery.
//!
//! A **specialist** node is parametric over the chain it serves: it announces a
//! [`ChainTag`] (kind + network name), so the mesh can run one node per
//! Solana-like network — Solana testnet, devnet, mainnet, Eclipse, Sonic, … —
//! plus EVM chains, with no single chain or broker at the centre.

use corrosive_agents::prelude::AgentIdentity;
use serde::{Deserialize, Serialize};

/// The one gossipsub topic the whole mesh talks on.
pub const TOPIC: &str = "corrosive-agents/solana-mesh/v1";

/// The JSON-RPC dialect a chain speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainKind {
    /// Solana Virtual Machine family (Solana clusters, Eclipse, Sonic, …):
    /// `getSlot` / `getGenesisHash` / `getBalance` / `requestAirdrop`.
    Svm,
    /// EVM family (Ethereum, Base, Polygon, …): `eth_blockNumber` / `eth_chainId`
    /// / `eth_getBalance`.
    Evm,
}

impl ChainKind {
    /// Stable lower-case identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            ChainKind::Svm => "svm",
            ChainKind::Evm => "evm",
        }
    }
}

/// A chain's identity on the wire: its dialect and a human network name
/// (e.g. `solana-testnet`, `eclipse`, `base`). The RPC endpoint is kept local
/// and never gossiped — so an API-keyed URL never leaves the node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainTag {
    /// Which JSON-RPC dialect the chain speaks.
    pub kind: ChainKind,
    /// The network's display name.
    pub name: String,
}

impl ChainTag {
    /// A log-friendly `name (kind)` label.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.kind.as_str())
    }
}

/// The role a node plays in the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The end user's entry point: sends a question to the orchestrator and
    /// prints the final answer. Runs no LLM — it only holds an identity.
    Client,
    /// Coordinates the mesh: fans the question out to every discovered chain
    /// specialist, then synthesises their on-chain facts into a final answer.
    Orchestrator,
    /// A chain specialist, parametric over the network it serves (see
    /// [`ChainTag`]). Reads its chain over JSON-RPC.
    Specialist,
}

impl Role {
    /// Stable lower-case identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Client => "client",
            Role::Orchestrator => "orchestrator",
            Role::Specialist => "specialist",
        }
    }

    /// Only the orchestrator drives an LLM (to synthesise). Specialists read
    /// chains directly; the client holds an identity only.
    pub fn wants_llm(self) -> bool {
        matches!(self, Role::Orchestrator)
    }

    /// Parse a *control* role. Specialist roles are named by their chain and
    /// resolved separately (see `chains::resolve_chain`).
    pub fn parse_control(raw: &str) -> Option<Role> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "client" | "user" | "caller" => Some(Role::Client),
            "orchestrator" | "coordinator" | "orchestra" => Some(Role::Orchestrator),
            _ => None,
        }
    }

    /// One-line description used as the agent's manifest description.
    pub fn description(self) -> &'static str {
        match self {
            Role::Client => "Sends a question to the orchestrator and shows the final answer.",
            Role::Orchestrator => "Coordinates the decentralised multi-chain agent mesh.",
            Role::Specialist => "Reads a blockchain over JSON-RPC on behalf of the mesh.",
        }
    }

    /// System prompt for roles that drive an LLM.
    pub fn system_prompt(self) -> &'static str {
        match self {
            Role::Orchestrator => {
                "You are the orchestrator of a decentralised, multi-chain blockchain agent mesh. \
                 You are given a user question together with freshly-read on-chain facts gathered \
                 by specialist peers, each labelled with the network it came from (e.g. \
                 solana-testnet, eclipse, ethereum). Synthesise ONE clear, correct, concise answer \
                 that attributes each figure to its network — slots, genesis hashes, block heights, \
                 chain ids and balances. Do not invent any data that is not in the facts."
            }
            _ => "",
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
    /// For a specialist: which chain it serves. `None` for client/orchestrator.
    pub chain: Option<ChainTag>,
    /// Seconds since the Unix epoch — purely for logging/freshness.
    pub ts: u64,
}

/// A unit of work addressed from the orchestrator to one specialist peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Correlation id; the matching [`TaskResult`] carries the same value.
    pub id: String,
    /// Peer id of the orchestrator (where the result should be sent back).
    pub origin: String,
    /// Peer id of the intended specialist (chosen from the discovery registry).
    pub target: String,
    /// The user's question.
    pub question: String,
    /// Orchestrator's did:key, so the specialist can authenticate the task.
    pub pubkey: String,
    /// Ed25519 signature over `(id, question)` by the orchestrator.
    pub sig: String,
}

/// A specialist's on-chain findings, addressed back to the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Correlation id (matches the originating [`Task::id`]).
    pub id: String,
    /// Peer id of the orchestrator this result is addressed to.
    pub target: String,
    /// Peer id of the specialist that produced it.
    pub from: String,
    /// Which chain the facts came from.
    pub chain: ChainTag,
    /// The gathered on-chain facts (plain text).
    pub content: String,
    /// The specialist's did:key, so the orchestrator can authenticate it.
    pub pubkey: String,
    /// Ed25519 signature over `(id, content)` by the specialist.
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
    /// Work request (orchestrator → specialist).
    Task(Task),
    /// Work result (specialist → orchestrator).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies_bound_to_id() {
        let id = AgentIdentity::generate();
        let (pubkey, sig) = sign(&id, "req-1", "hello world");
        assert_eq!(pubkey, id.did_key());
        assert!(verify(&pubkey, "req-1", "hello world", &sig).is_ok());
        // A different id or body must fail (the signature binds both).
        assert!(verify(&pubkey, "req-2", "hello world", &sig).is_err());
        assert!(verify(&pubkey, "req-1", "tampered", &sig).is_err());
    }

    #[test]
    fn signing_message_binds_id_and_body() {
        assert_eq!(signing_message("abc", "body"), "abc\nbody");
    }

    #[test]
    fn short_takes_the_suffix() {
        assert_eq!(short("1234567890"), "34567890");
        assert_eq!(short("short"), "short");
    }

    #[test]
    fn roles_parse_and_describe() {
        assert_eq!(Role::parse_control("client"), Some(Role::Client));
        assert_eq!(Role::parse_control("USER"), Some(Role::Client));
        assert_eq!(Role::parse_control("orchestrator"), Some(Role::Orchestrator));
        assert_eq!(Role::parse_control("coordinator"), Some(Role::Orchestrator));
        // A chain name is not a control role.
        assert_eq!(Role::parse_control("solana-testnet"), None);

        assert_eq!(Role::Client.as_str(), "client");
        assert_eq!(Role::Specialist.as_str(), "specialist");
        assert!(Role::Orchestrator.wants_llm());
        assert!(!Role::Specialist.wants_llm());
        assert!(!Role::Client.wants_llm());

        assert!(!Role::Specialist.description().is_empty());
        assert!(!Role::Orchestrator.system_prompt().is_empty());
        assert!(Role::Client.system_prompt().is_empty());
        assert!(Role::Specialist.system_prompt().is_empty());
    }

    #[test]
    fn chain_kind_and_tag_labels() {
        assert_eq!(ChainKind::Svm.as_str(), "svm");
        assert_eq!(ChainKind::Evm.as_str(), "evm");
        let tag = ChainTag { kind: ChainKind::Svm, name: "eclipse".to_string() };
        assert_eq!(tag.label(), "eclipse (svm)");
    }

    fn roundtrip(env: &Envelope) -> Envelope {
        let bytes = serde_json::to_vec(env).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn envelopes_round_trip_through_json() {
        let tag = ChainTag { kind: ChainKind::Svm, name: "solana-testnet".to_string() };

        let announce = Envelope::Announce(Announce {
            peer: "peerA".into(),
            role: Role::Specialist,
            name: "specialist".into(),
            pubkey: "did:key:zTest".into(),
            chain: Some(tag.clone()),
            ts: 42,
        });
        match roundtrip(&announce) {
            Envelope::Announce(a) => {
                assert_eq!(a.peer, "peerA");
                assert_eq!(a.chain.unwrap().name, "solana-testnet");
            }
            _ => panic!("wrong variant"),
        }

        let req = Envelope::Request(ClientRequest {
            id: "r1".into(),
            from: "c".into(),
            target: "o".into(),
            question: "q?".into(),
            pubkey: "k".into(),
            sig: "s".into(),
        });
        assert!(matches!(roundtrip(&req), Envelope::Request(_)));

        let resp = Envelope::Response(ClientResponse {
            id: "r1".into(),
            target: "c".into(),
            from: "o".into(),
            answer: "a".into(),
            pubkey: "k".into(),
            sig: "s".into(),
        });
        assert!(matches!(roundtrip(&resp), Envelope::Response(_)));

        let task = Envelope::Task(Task {
            id: "r1".into(),
            origin: "o".into(),
            target: "s".into(),
            question: "q?".into(),
            pubkey: "k".into(),
            sig: "s".into(),
        });
        assert!(matches!(roundtrip(&task), Envelope::Task(_)));

        let result = Envelope::Result(TaskResult {
            id: "r1".into(),
            target: "o".into(),
            from: "s".into(),
            chain: tag,
            content: "facts".into(),
            pubkey: "k".into(),
            sig: "s".into(),
        });
        match roundtrip(&result) {
            Envelope::Result(r) => assert_eq!(r.content, "facts"),
            _ => panic!("wrong variant"),
        }
    }
}
