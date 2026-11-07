#!/usr/bin/env bash
# Launch one node of the decentralised multi-chain agent mesh (one terminal per
# role). A specialist's role name IS the network it serves.
#
#   ./run_role.sh orchestrator                  # start this FIRST (the seed)
#   ./run_role.sh solana-testnet                # a Solana testnet specialist
#   ./run_role.sh eclipse                       # another Solana-like (SVM) chain
#   ./run_role.sh sonic-testnet                 # another Solana-like (SVM) chain
#   ./run_role.sh ethereum                      # an EVM chain (bonus)
#   ./run_role.sh client ["your question"]
#   ./run_role.sh solo   ["your question"]      # single-process tool-calling demo
#
# Presets: solana-mainnet solana-testnet solana-devnet eclipse sonic-testnet
#          ethereum base polygon
# Custom chain:  CHAIN_KIND=svm RPC_URL=https://… ./run_role.sh my-svm-chain
set -euo pipefail
cd "$(dirname "$0")"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# Load NVIDIA_API_KEY (and any overrides) from the repository-root .env.
[ -f ../../.env ] && { set -a; . ../../.env; set +a; }
export RUST_LOG="${RUST_LOG:-warn,solana_agents=info,libp2p_mdns=off}"

SEED="/ip4/127.0.0.1/tcp/4201"
BIN="target/release/solana_agents"
[ -x "$BIN" ] || { echo "build first:  cargo build --release" >&2; exit 1; }

role="${1:?usage: run_role.sh <orchestrator|client|solo|CHAIN> [question]}"
QUESTION="${2:-${QUESTION:-}}"   # client/solo only; else the binary's default

case "$role" in
  # The orchestrator binds a fixed port and acts as the bootstrap seed so the
  # others can find it even where mDNS multicast is blocked.
  orchestrator) exec env ROLE=orchestrator LISTEN="$SEED" "$BIN" ;;
  client)       exec env ROLE=client QUESTION="$QUESTION" BOOTSTRAP="$SEED" "$BIN" ;;
  solo)         exec env ROLE=solo QUESTION="$QUESTION" "$BIN" ;;
  # Anything else is a chain specialist; ROLE names the network.
  *)            exec env ROLE="$role" BOOTSTRAP="$SEED" "$BIN" ;;
esac
