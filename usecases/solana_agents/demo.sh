#!/usr/bin/env bash
#
# One-stream demo of the decentralised, multi-chain on-chain agent mesh.
#
# Launches the orchestrator (bootstrap seed) plus one specialist per network in
# $CHAINS, then runs the CLIENT in the foreground. The client discovers the
# orchestrator over gossip, sends a signed question, and the mesh runs:
#   client → orchestrator → (one specialist per chain, in parallel) → synthesize → client
#
# Usage:
#   ./demo.sh ["your question"]
#   CHAINS="solana-testnet eclipse" ./demo.sh
set -euo pipefail
cd "$(dirname "$0")"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

BIN="target/release/solana_agents"
SEED="/ip4/127.0.0.1/tcp/4201"
# Default: Solana testnet + two other Solana-like (SVM) chains + an EVM chain.
CHAINS="${CHAINS:-solana-testnet eclipse sonic-testnet ethereum}"
QUESTION="${1:-${QUESTION:-For each network you can reach, report the current slot/block and its genesis hash or chain id, and confirm they are different networks.}}"
LOGDIR="$(mktemp -d)"

[ -f ../../.env ] && { set -a; . ../../.env; set +a; }
export RUST_LOG="${RUST_LOG:-warn,solana_agents=info,libp2p_mdns=off}"
[ -x "$BIN" ] || { echo "build first:  cargo build --release" >&2; exit 1; }

printf '\n\033[1;36m╔══════════════════════════════════════════════════════════════╗\033[0m\n'
printf   '\033[1;36m║   corrosive_agents · decentralised multi-chain agent mesh    ║\033[0m\n'
printf   '\033[1;36m╚══════════════════════════════════════════════════════════════╝\033[0m\n'
printf '  \033[1mTransport :\033[0m libp2p  (TCP · noise · yamux · gossipsub · mDNS)\n'
printf '  \033[1mNetworks  :\033[0m %s\n' "$CHAINS"
if [ -z "${NVIDIA_API_KEY:-}" ]; then
  printf '  \033[1mLLM       :\033[0m \033[33mnone — orchestrator will print raw on-chain facts\033[0m\n'
else
  printf '  \033[1mLLM       :\033[0m %s  \033[2m(free NVIDIA NIM)\033[0m\n' "${NVIDIA_MODEL:-nvidia/nemotron-3.5-bolt-e4b}"
fi
printf '  \033[1mFlow      :\033[0m client → orchestrator → (one specialist per chain) → synthesize → client\n\n'

PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT INT TERM

start() { # start <role> <extra-env...>
  local role="$1"; shift
  env ROLE="$role" "$@" "$BIN" >"$LOGDIR/$role.log" 2>&1 &
  PIDS+=("$!")
  printf '  \033[32m▸\033[0m spawned \033[1m%-16s\033[0m node  \033[2m(logs: %s/%s.log)\033[0m\n' "$role" "$LOGDIR" "$role"
}

printf '\033[2m── spawning the mesh (orchestrator seed + one specialist per chain) ─\033[0m\n'
start orchestrator LISTEN="$SEED"
sleep 1
for chain in $CHAINS; do start "$chain" BOOTSTRAP="$SEED"; done
sleep 3
printf '\n\033[2m── client joining mesh, discovering the orchestrator, asking ──\033[0m\n'

env ROLE=client QUESTION="$QUESTION" BOOTSTRAP="$SEED" "$BIN" </dev/null

printf '\n\033[1;32m✔  Demo complete — libp2p agents cooperated across multiple independent chains, no broker.\033[0m\n\n'
