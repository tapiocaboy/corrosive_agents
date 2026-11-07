//! The libp2p layer: a swarm with mDNS discovery and gossipsub messaging,
//! wrapped behind a small channel-based [`NodeHandle`].
//!
//! The swarm runs in its own task. The rest of the program never touches
//! libp2p types directly — it publishes [`Envelope`]s through a [`Publisher`]
//! and receives them from the handle's inbox.

use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, mdns, noise, tcp, yamux, Multiaddr, PeerId};
use tokio::sync::mpsc;

use crate::protocol::{Announce, ChainTag, Envelope, Role, TOPIC};

/// Composed behaviour: gossip + local discovery.
#[derive(NetworkBehaviour)]
struct AgentBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

/// A cheap, cloneable publish handle onto the mesh.
#[derive(Clone)]
pub struct Publisher {
    tx: mpsc::UnboundedSender<Envelope>,
    /// This node's own peer id.
    pub peer_id: PeerId,
}

impl Publisher {
    /// Queue an envelope for broadcast. Fire-and-forget: if the mesh has no
    /// peers yet the swarm drops it (and the caller retries).
    pub fn publish(&self, env: Envelope) {
        let _ = self.tx.send(env);
    }
}

/// Owns the inbound side of the mesh and hands out [`Publisher`]s.
pub struct NodeHandle {
    /// This node's own peer id.
    pub peer_id: PeerId,
    out_tx: mpsc::UnboundedSender<Envelope>,
    inbox: mpsc::UnboundedReceiver<Envelope>,
}

impl NodeHandle {
    /// A cloneable publish handle.
    pub fn publisher(&self) -> Publisher {
        Publisher {
            tx: self.out_tx.clone(),
            peer_id: self.peer_id,
        }
    }

    /// Receive the next decoded inbound envelope (None once the swarm stops).
    pub async fn recv(&mut self) -> Option<Envelope> {
        self.inbox.recv().await
    }
}

/// Boot a swarm using `LISTEN` / `BOOTSTRAP` from the environment.
pub async fn start() -> Result<NodeHandle> {
    let listen = std::env::var("LISTEN").unwrap_or_else(|_| "/ip4/0.0.0.0/tcp/0".to_string());
    let bootstrap: Vec<String> = std::env::var("BOOTSTRAP")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    start_with(&listen, &bootstrap).await
}

/// Boot a swarm with an explicit listen multiaddr and bootstrap seed list,
/// start listening, and spawn its event loop. (`start` is the env-driven wrapper.)
pub async fn start_with(listen: &str, bootstrap: &[String]) -> Result<NodeHandle> {
    // Secure key creation: a fresh Ed25519 keypair is the node's libp2p
    // identity. Its public key *is* the peer id, and it keys the noise
    // handshake that encrypts every connection.
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let node_peer = keypair.public().to_peer_id();
    tracing::info!(
        "🔑 secure key creation — libp2p node key (Ed25519) generated; peer id {}",
        node_peer
    );

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(1))
                .validation_mode(gossipsub::ValidationMode::Strict)
                // Facts + answers can be a few KB; give the pipe plenty of room.
                .max_transmit_size(512 * 1024)
                .build()
                .map_err(std::io::Error::other)?;
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;
            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?;
            Ok(AgentBehaviour { gossipsub, mdns })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    let topic = gossipsub::IdentTopic::new(TOPIC);
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    // Listen address (override with LISTEN, e.g. a fixed port for bootstrapping).
    let listen_addr: Multiaddr = listen
        .parse()
        .with_context(|| format!("invalid LISTEN multiaddr: {listen}"))?;
    swarm.listen_on(listen_addr)?;

    // Optional explicit bootstrap peers for environments where mDNS multicast
    // is unavailable (restricted Wi-Fi/VPN, some CI). Discovery still flows
    // over gossip once connected — these are only a dialing seed, not a broker.
    for addr in bootstrap.iter().map(String::as_str) {
        match addr.parse::<Multiaddr>() {
            Ok(ma) => {
                tracing::info!("dialing bootstrap peer {ma}");
                if let Err(e) = swarm.dial(ma) {
                    tracing::warn!("bootstrap dial failed: {e}");
                }
            }
            Err(e) => tracing::warn!("ignoring invalid BOOTSTRAP multiaddr '{addr}': {e}"),
        }
    }

    let peer_id = *swarm.local_peer_id();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Envelope>();
    let (in_tx, inbox) = mpsc::unbounded_channel::<Envelope>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                outbound = out_rx.recv() => {
                    let Some(env) = outbound else { break };
                    match serde_json::to_vec(&env) {
                        Ok(bytes) => {
                            if let Err(e) = swarm
                                .behaviour_mut()
                                .gossipsub
                                .publish(topic.clone(), bytes)
                            {
                                // Most commonly `InsufficientPeers` before the
                                // mesh forms — harmless; the caller retries.
                                tracing::debug!("gossip publish skipped: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("failed to encode envelope: {e}"),
                    }
                }
                event = swarm.select_next_some() => match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        tracing::info!("listening on {address}");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        tracing::info!(
                            "🔒 encrypted P2P channel up (noise XX · yamux) with {}",
                            crate::protocol::short(&peer_id.to_string())
                        );
                        // Works for both mDNS- and bootstrap-dialed peers.
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    }
                    SwarmEvent::Behaviour(AgentBehaviourEvent::Mdns(
                        mdns::Event::Discovered(peers),
                    )) => {
                        for (peer, addr) in peers {
                            tracing::info!("mDNS discovered {peer} at {addr}");
                            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                            // Dial explicitly so the gossip connection is
                            // established even if we are not already linked.
                            if let Err(e) = swarm.dial(addr) {
                                tracing::debug!("dial {peer} skipped: {e}");
                            }
                        }
                    }
                    SwarmEvent::Behaviour(AgentBehaviourEvent::Mdns(
                        mdns::Event::Expired(peers),
                    )) => {
                        for (peer, _addr) in peers {
                            tracing::info!("mDNS peer expired {peer}");
                            swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer);
                        }
                    }
                    SwarmEvent::Behaviour(AgentBehaviourEvent::Gossipsub(
                        gossipsub::Event::Message { message, .. },
                    )) => match serde_json::from_slice::<Envelope>(&message.data) {
                        Ok(env) => {
                            if in_tx.send(env).is_err() {
                                break; // receiver gone
                            }
                        }
                        Err(e) => tracing::debug!("ignoring undecodable gossip message: {e}"),
                    },
                    _ => {}
                }
            }
        }
        tracing::info!("swarm event loop stopped");
    });

    Ok(NodeHandle {
        peer_id,
        out_tx,
        inbox,
    })
}

/// Broadcast a presence announcement for `role` every 5 seconds forever.
///
/// `pubkey` is the node's agent identity as a `did:key`; peers pin it and
/// verify every signed message this node sends against it. `chain` is the
/// network a specialist serves (`None` for the client/orchestrator).
pub fn spawn_announcer(publisher: Publisher, role: Role, pubkey: String, chain: Option<ChainTag>) {
    tokio::spawn(async move {
        loop {
            publisher.publish(Envelope::Announce(Announce {
                peer: publisher.peer_id.to_string(),
                role,
                name: role.as_str().to_string(),
                pubkey: pubkey.clone(),
                chain: chain.clone(),
                ts: now_secs(),
            }));
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
