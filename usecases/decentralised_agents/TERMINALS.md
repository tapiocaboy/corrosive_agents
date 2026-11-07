# Run the mesh by hand — one terminal per agent

This walks you through running the decentralised agent mesh as **six separate
terminals**, where **each terminal is one independent libp2p node**: the four
specialists, the orchestrator, and the client. You'll watch, live and per
terminal, the secure key creation, encrypted peer-to-peer connections, peer
discovery, signed message passing, **real NVIDIA LLM calls**, and the final
answer.

> Prefer one window split into six panes? Run `./demo_mesh.sh` instead (needs
> `tmux`). This doc is the manual, one-terminal-per-agent version.

## 0. One-time setup

```sh
# from the repository root
cd usecases/decentralised_agents
cargo build --release

# put a FREE NVIDIA NIM key in the repository-root .env (loaded automatically)
echo 'NVIDIA_API_KEY=nvapi-...' >> ../../.env   # get one at https://build.nvidia.com
```

All six terminals start from this directory. The router listens on a fixed
address so the others can dial it as a bootstrap seed (this makes discovery work
even where mDNS multicast is blocked); discovery still flows over gossip, so
there is no central broker.

Each command sets `RUST_LOG=decentralised_agents=info` so you see this crate's
logs (keys, discovery, messages, NVIDIA calls) without libp2p's internal noise.

## The six terminals

Start them **top to bottom**. Give terminal 1 a second to bind before the rest;
start the **client last**.

### Terminal 1 — ROUTER  (also the bootstrap seed)
```sh
RUST_LOG=decentralised_agents=info \
NVIDIA_MODEL=nvidia/nemotron-3.5-bolt-e4b \
ROLE=router LISTEN=/ip4/127.0.0.1/tcp/4121 \
cargo run --release
```

### Terminal 2 — QNA
```sh
RUST_LOG=decentralised_agents=info \
NVIDIA_MODEL=nvidia/nemotron-3.5-bolt-e4b \
ROLE=qna BOOTSTRAP=/ip4/127.0.0.1/tcp/4121 \
cargo run --release
```

### Terminal 3 — REASONING
```sh
RUST_LOG=decentralised_agents=info \
NVIDIA_MODEL=nvidia/nemotron-3.5-bolt-e4b \
ROLE=reasoning BOOTSTRAP=/ip4/127.0.0.1/tcp/4121 \
cargo run --release
```

### Terminal 4 — SYNTHESIZER
```sh
RUST_LOG=decentralised_agents=info \
NVIDIA_MODEL=nvidia/nemotron-3.5-bolt-e4b \
ROLE=synthesizer BOOTSTRAP=/ip4/127.0.0.1/tcp/4121 \
cargo run --release
```

### Terminal 5 — ORCHESTRATOR
```sh
RUST_LOG=decentralised_agents=info \
NVIDIA_MODEL=nvidia/nemotron-3.5-bolt-e4b \
ROLE=orchestrator BOOTSTRAP=/ip4/127.0.0.1/tcp/4121 \
cargo run --release
```

### Terminal 6 — CLIENT  (asks the question)
```sh
RUST_LOG=decentralised_agents=info \
ROLE=client QUESTION="What is the Raft consensus algorithm, and when should I choose it over Paxos?" \
BOOTSTRAP=/ip4/127.0.0.1/tcp/4121 \
cargo run --release
```

The client has no `NVIDIA_MODEL` and needs no key — it runs no LLM, only an
identity. Change `QUESTION="..."` to ask anything.

## What you'll see in each terminal

| Terminal | What its log shows |
|----------|--------------------|
| **ROUTER / QNA / REASONING / SYNTHESIZER** | `🔑 secure key creation` (Ed25519 identity + libp2p node key) → `🔒 encrypted P2P channel up (noise XX)` → `🔎 discovered peer … role=…` → `📥 received … task` + `🔏 verified orchestrator signature ✔` → **`🧠 LIVE LLM call → POST https://integrate.api.nvidia.com/v1/chat/completions`** → **`🌐 NVIDIA NIM responded … tokens prompt=… completion=… total=…`** → `📤 publishing signed result`. The synthesizer also prints the **FINAL ANSWER** block. |
| **ORCHESTRATOR** | discovery of every role, `✅ mesh formed`, `📥 client request … 🔏 verified client signature ✔`, the three pipeline stages (`➡️ Route`, `➡️ Answer ∥ Reason`, `➡️ Synthesize`), each worker result verified + key-pinned, then `📤 signing final answer → client`. |
| **CLIENT** | `🙋 client online`, `🔎 discovered orchestrator`, `📤 sending signed request`, then `📥 final answer received`, `🔏 verified orchestrator signature ✔`, and the **✅ FINAL ANSWER (verified)** block. |

### Proof the LLM calls are real
Every specialist terminal prints, for each call:

```
🧠 LIVE LLM call → POST https://integrate.api.nvidia.com/v1/chat/completions · model=nvidia/nemotron-3.5-bolt-e4b · 36 chars in …awaiting NVIDIA
🌐 NVIDIA NIM responded (integrate.api.nvidia.com) · served model=nvidia/nemotron-3.5-bolt-e4b · tokens prompt=90 completion=38 total=128
✅ NVIDIA reply ← 187 chars in 1279 ms (real API round-trip)
```

The `tokens prompt/completion/total` counts come straight from NVIDIA's HTTP
response (via the SDK's usage hook) and differ on every call — they cannot be
faked by a mock.

## Stopping

The client and orchestrator exit on their own once the answer is delivered
(the orchestrator uses `ORCHESTRATOR_ONESHOT` in the scripted demos; run by
hand it stays up to serve more requests — press `Ctrl-C`). Stop the four
specialists with `Ctrl-C` in each terminal.

## Troubleshooting

- **`set NVIDIA_API_KEY`** — the key isn't in `../../.env`. Add it (see setup).
- **A model id returns 404/410** — the NIM catalog drifts; set `NVIDIA_MODEL`
  to an id confirmed callable by your key (the default `nemotron-3.5-bolt-e4b`
  was verified 2026-11-07). Probe:
  ```sh
  curl https://integrate.api.nvidia.com/v1/chat/completions \
    -H "Authorization: Bearer $NVIDIA_API_KEY" -H "Content-Type: application/json" \
    -d '{"model":"nvidia/nemotron-3.5-bolt-e4b","messages":[{"role":"user","content":"hi"}],"max_tokens":16}'
  ```
- **`error sending packet … No route to host`** — harmless; that's mDNS
  multicast being blocked. Discovery falls back to the `BOOTSTRAP` seed. Add
  `libp2p_mdns=off` to `RUST_LOG` to hide it.
