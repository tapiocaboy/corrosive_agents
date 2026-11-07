#!/usr/bin/env bash
# Launch one role of the mesh with the shared demo environment.
# Used by demo_mesh.sh to populate each tmux pane.  Usage: ./run_role.sh <role>
set -euo pipefail
cd "$(dirname "$0")"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

[ -f ../../.env ] && { set -a; . ../../.env; set +a; }
export NVIDIA_MODEL="${NVIDIA_MODEL:-nvidia/nemotron-3.5-bolt-e4b}"
export RUST_LOG="${RUST_LOG:-warn,decentralised_agents=info,libp2p_mdns=off}"

SEED="/ip4/127.0.0.1/tcp/4121"
BIN="target/release/decentralised_agents"
role="${1:?usage: run_role.sh <role> [question]}"
# The question (client only); $2, else $QUESTION, else a built-in default. We
# avoid the name PROMPT — it collides with the shell's own $PROMPT in zsh.
QUESTION="${2:-${QUESTION:-What is the Raft consensus algorithm, and when should I choose it over Paxos?}}"

# Stagger startup so the seed binds first and the flow reads top-to-bottom.
case "$role" in
  router)       exec env ROLE=router LISTEN="$SEED" "$BIN" ;;
  orchestrator) sleep 1; exec env ORCHESTRATOR_ONESHOT=1 ROLE=orchestrator BOOTSTRAP="$SEED" "$BIN" ;;
  client)       sleep 3; exec env ROLE=client QUESTION="$QUESTION" BOOTSTRAP="$SEED" "$BIN" ;;
  *)            sleep 1; exec env ROLE="$role" BOOTSTRAP="$SEED" "$BIN" ;;
esac
