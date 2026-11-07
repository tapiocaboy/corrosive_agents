#!/usr/bin/env bash
#
# Single-stream demonstration of the decentralised agent mesh.
#
# Launches the five mesh nodes in the background (router is the bootstrap seed;
# orchestrator + 3 specialists) and runs the CLIENT in the foreground. The
# client discovers the orchestrator over gossip, sends a signed request, and
# the pipeline runs: Route → (Answer ∥ Reason) → Synthesize → back to client.
#
# For the full six-pane split-terminal view, use ./demo_mesh.sh instead.
#
# Usage:  ./demo.sh ["your question"]
set -euo pipefail

cd "$(dirname "$0")"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

BIN="target/release/decentralised_agents"
SEED="/ip4/127.0.0.1/tcp/4101"
MODEL="${NVIDIA_MODEL:-nvidia/nemotron-3.5-bolt-e4b}"   # fast, free, returns content directly
QUESTION="${1:-${QUESTION:-What is the Raft consensus algorithm, and when should I choose it over Paxos?}}"
LOGDIR="$(mktemp -d)"

if [ -f ../../.env ]; then set -a; . ../../.env; set +a; fi
: "${NVIDIA_API_KEY:?set NVIDIA_API_KEY (free key at https://build.nvidia.com)}"

export NVIDIA_MODEL="$MODEL"
export ORCHESTRATOR_ONESHOT=1
export RUST_LOG="${RUST_LOG:-warn,decentralised_agents=info,libp2p_mdns=off}"

if [ ! -x "$BIN" ]; then echo "build first:  cargo build --release" >&2; exit 1; fi

printf '\n\033[1;36m╔══════════════════════════════════════════════════════════════╗\033[0m\n'
printf   '\033[1;36m║   corrosive_agents · decentralised agent mesh (libp2p)       ║\033[0m\n'
printf   '\033[1;36m╚══════════════════════════════════════════════════════════════╝\033[0m\n'
printf '  \033[1mTransport :\033[0m libp2p  (TCP · noise · yamux · gossipsub · mDNS)\n'
printf '  \033[1mModel     :\033[0m %s  \033[2m(free NVIDIA NIM)\033[0m\n' "$MODEL"
printf '  \033[1mFlow      :\033[0m client → orchestrator → router → (qna ∥ reasoning) → synthesizer → client\n\n'

PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT INT TERM

start() { # start <role> <extra-env...>
  local role="$1"; shift
  env ROLE="$role" "$@" "$BIN" >"$LOGDIR/$role.log" 2>&1 &
  PIDS+=("$!")
  printf '  \033[32m▸\033[0m spawned \033[1m%-12s\033[0m node  \033[2m(logs: %s/%s.log)\033[0m\n' "$role" "$LOGDIR" "$role"
}

printf '\033[2m── spawning the mesh (5 background nodes) ─────────────────────\033[0m\n'
start router       LISTEN="$SEED"
start qna          BOOTSTRAP="$SEED"
start reasoning    BOOTSTRAP="$SEED"
start synthesizer  BOOTSTRAP="$SEED"
start orchestrator BOOTSTRAP="$SEED"
printf '\n\033[2m── client joining mesh, discovering the orchestrator, asking ──\033[0m\n'

# Client in the foreground: discovers the orchestrator, sends a signed request,
# prints the verified final answer, and exits.
env ROLE=client QUESTION="$QUESTION" BOOTSTRAP="$SEED" "$BIN" </dev/null

printf '\n\033[1;32m✔  Demo complete — six libp2p agents cooperated with no central broker.\033[0m\n\n'
