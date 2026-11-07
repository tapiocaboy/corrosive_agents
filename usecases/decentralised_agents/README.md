# Decentralised agents — a client, an orchestrator, and a libp2p specialist mesh

A use case for [`corrosive_agents`](../../README.md): **a client asks a
question; an orchestrator discovers a mesh of specialist agents over libp2p and
coordinates them to answer it — with no central broker, and every message
cryptographically signed.**

- **Discovery** — libp2p **mDNS** (local-IP multicast) with a **bootstrap**
  fallback for networks that block multicast. No seed list, no registry.
- **Coordination** — libp2p **gossipsub**. Presence announcements *and*
  request/task/result messages travel over one gossip topic; messages are
  addressed by peer id and correlated by request id.
- **Security** — each node generates an **Ed25519 agent identity** (a
  `did:key`) at startup, advertises it in its announcements, and **signs every
  message** it sends. Receivers verify the signature and confirm the key
  matches the one pinned at discovery. The libp2p transport itself is encrypted
  with a **noise XX** handshake over an Ed25519 node key.
- **Brains** — each specialist is a `corrosive_agents::Agent` driven by an
  `NvidiaClient` (a free NVIDIA NIM model), with a role-specific system prompt.
  The client runs no LLM — it holds an identity only.

> This is a self-contained crate with its **own** `[workspace]` and lockfile,
> so its heavy libp2p dependency tree never touches the published
> `corrosive_agents` crate or its CI. It depends on the library via
> `path = "../.."` with `default-features = false`.

## The six roles

| Role          | `ROLE` value  | Job |
|---------------|---------------|-----|
| Client        | `client`      | Sends a signed request to the orchestrator; prints the verified final answer. No LLM. |
| Orchestrator  | `orchestrator`| Discovers peers, drives the pipeline, returns the final answer to the client. |
| Router        | `router`      | Classifies the query and sketches a plan. |
| QnA           | `qna`         | Answers factually and concisely. |
| Reasoning     | `reasoning`   | Works the problem step by step. |
| Synthesizer   | `synthesizer` | Merges the peer outputs into one final answer (and prints it). |

## The flow (every hop is a signed gossipsub message)

```
  client ──Request──▶ orchestrator
                          │  discovers peers over gossip, then:
                          ├─Route─────▶ router       → plan
                          ├─Answer────▶ qna          → factual answer   ∥ concurrent
                          ├─Reason────▶ reasoning    → analysis
                          └─Synthesize▶ synthesizer  → FINAL ANSWER
  client ◀─Response── orchestrator
```

The orchestrator only addresses a task to a role it has actually *discovered*,
so routing is driven by live gossip state, not configuration. The synthesizer
prints the final answer in its own pane; the orchestrator returns it to the
client, which verifies the signature and prints it too.

## Watch it: the six-pane split-terminal demo

The headline demo runs all six nodes in a **tmux** split terminal — one pane
each — so you can watch, per node, the secure key creation, encrypted P2P
connections, peer discovery, signed message passing, LLM calls, and the final
answer.

```sh
cargo build --release
./demo_mesh.sh "What is a Merkle tree and why is it useful?"
```

```
┌──────────────┬───────────────┐
│ 1 CLIENT     │ 2 ORCHESTRATOR│
├──────────────┼───────────────┤
│ 3 ROUTER     │ 4 QNA         │
├──────────────┼───────────────┤
│ 5 REASONING  │ 6 SYNTHESIZER │
└──────────────┴───────────────┘
```

Needs `tmux` (`brew install tmux`). Tear down with `./demo_mesh.sh kill`. For a
readable, step-by-step run (handy for recording), pace the pipeline:

```sh
DEMO_PACE_MS=1800 ./demo_mesh.sh "What is a Merkle tree and why is it useful?"
```

### Recordings

Pre-rendered videos live in `recordings/`:

- **`decentralised_mesh_narrated.mp4`** — a ~2½-minute **narrated walkthrough**:
  an intro, each terminal's setup (client first), the live run, and the client's
  verified answer, with a spoken voice-over of what each terminal is doing.
- **`decentralised_mesh_6pane.{mp4,gif}`** — the raw six-pane run.

Re-record the six-pane run with [`vhs`](https://github.com/charmbracelet/vhs):
`vhs demo_mesh.tape`.

### Single-stream version

For one combined stream (no tmux) — five nodes in the background, the client in
the foreground:

```sh
./demo.sh "What is the Raft consensus algorithm, and when should I choose it over Paxos?"
```

## Run it with Docker (6 containers)

Put your key in the repository-root `.env` (next to the crate's `Cargo.toml`):

```sh
echo 'NVIDIA_API_KEY=nvapi-...' >> ../../.env   # free key: https://build.nvidia.com
```

Then, from this directory:

```sh
docker compose up --build
QUESTION="Explain CRDTs to a backend engineer" docker compose up --build   # your own question
docker compose logs -f client                                              # follow the answer
docker compose down                                                        # tear down
```

## Run it by hand (6 terminals — one per agent)

See **[TERMINALS.md](TERMINALS.md)** for the full step-by-step walkthrough (what
each terminal shows, incl. the real NVIDIA call + token-usage lines). The short
version — from this directory, with `NVIDIA_API_KEY` in `../../.env`:

```sh
SEED=/ip4/127.0.0.1/tcp/4121
ROLE=router       LISTEN=$SEED      cargo run --release   # terminal 1 (seed)
ROLE=qna          BOOTSTRAP=$SEED   cargo run --release   # terminal 2
ROLE=reasoning    BOOTSTRAP=$SEED   cargo run --release   # terminal 3
ROLE=synthesizer  BOOTSTRAP=$SEED   cargo run --release   # terminal 4
ROLE=orchestrator BOOTSTRAP=$SEED   cargo run --release   # terminal 5
ROLE=client QUESTION="What is a bloom filter?" BOOTSTRAP=$SEED cargo run --release   # terminal 6
```

## Configuration

| Env var          | Where            | Meaning |
|------------------|------------------|---------|
| `NVIDIA_API_KEY` | required (LLM roles) | NVIDIA NIM key. The `client` role does not need it. |
| `ROLE`           | required         | Which node this process is (see table above). |
| `QUESTION`       | client           | The question to ask. Defaults to a built-in demo. (Named `QUESTION`, not `PROMPT`, to avoid colliding with the shell's `$PROMPT`.) |
| `NVIDIA_MODEL`   | optional         | Override the chat model for every specialist. |
| `RUST_LOG`       | optional         | Log filter, e.g. `info,decentralised_agents=debug`. |
| `LISTEN`         | optional         | libp2p listen multiaddr (default `/ip4/0.0.0.0/tcp/0`). |
| `BOOTSTRAP`      | optional         | Comma-separated multiaddrs to dial when mDNS is unavailable. |
| `ORCHESTRATOR_ONESHOT` | optional   | Orchestrator serves one request, then exits (used by the demos). |

> **Model availability (read this):** `/v1/models` lists the whole global NIM
> catalog, but most of those ids return `404 "Not found for account"` (or `410`
> once retired) on your key, and the catalog drifts monthly. The demos use
> `nvidia/nemotron-3.5-bolt-e4b` (fast, free, verified callable 2026-11-07).
> Probe before trusting an id:
>
> ```sh
> curl https://integrate.api.nvidia.com/v1/chat/completions \
>   -H "Authorization: Bearer $NVIDIA_API_KEY" -H "Content-Type: application/json" \
>   -d '{"model":"<id>","messages":[{"role":"user","content":"hi"}],"max_tokens":16}'
> ```

## How it fits together

| File                 | Responsibility |
|----------------------|----------------|
| `src/main.rs`        | Parse `ROLE`, build the agent, start the node, dispatch. |
| `src/node.rs`        | libp2p swarm (keygen, noise, mDNS + gossipsub) behind a channel-based `NodeHandle`. |
| `src/protocol.rs`    | Roles, pipeline stages, signing helpers, and the signed JSON `Envelope` wire types. |
| `src/client.rs`      | Discover the orchestrator, send a signed request, verify + print the answer. |
| `src/roles.rs`       | Agent construction + the specialist worker loop (verify task, call LLM, sign result). |
| `src/orchestrator.rs`| Discovery registry + key pinning, request/response correlation, the pipeline. |
| `demo_mesh.sh` / `.tape` | Six-pane tmux demo + its VHS recording script. |
| `demo.sh`            | Single-stream (no-tmux) demo. |

### Reliability & security notes

- gossipsub is broadcast, so each message carries an explicit `target` peer id
  and role; nodes ignore anything not addressed to them.
- A publish before the mesh forms returns `InsufficientPeers` and is dropped;
  the orchestrator re-broadcasts each task every 3 s, and the client re-sends
  its request every 2 s, until the result arrives. Workers cache results per
  task id, so a re-broadcast never causes a second LLM call.
- Every `Request`, `Task`, `Result`, and `Response` is signed with the sender's
  Ed25519 agent identity and verified by the receiver against the `did:key`
  that peer advertised at discovery. This is application-layer authenticity on
  top of gossipsub's own message signing and the noise-encrypted transport.
- The callable NIM models are often *reasoning* models that place their final
  text in `message.content` only once generation finishes; `bolt-e4b` returns
  content directly, which is why the demos default to it.
