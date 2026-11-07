#!/usr/bin/env bash
#
# Six-pane split-terminal demonstration of the decentralised agent mesh.
#
#   ┌──────────────┬──────────────┐
#   │ 1 CLIENT     │ 2 ORCHESTRATOR│   client → orchestrator request
#   ├──────────────┼──────────────┤
#   │ 3 ROUTER     │ 4 QNA        │   orchestrator fans work out…
#   ├──────────────┼──────────────┤
#   │ 5 REASONING  │ 6 SYNTHESIZER│   …synthesizer produces the final answer
#   └──────────────┴──────────────┘
#
# Each pane is one independent libp2p node. You can watch, per pane, the secure
# key creation, encrypted P2P connections, peer discovery, signed message
# passing, LLM calls, and the final answer.
#
# Usage:
#   ./demo_mesh.sh ["your question"]   # set up + attach (what you want)
#   ./demo_mesh.sh --no-attach ["q"]   # set up detached (for scripting/CI)
#   ./demo_mesh.sh kill                # tear the session down
set -euo pipefail
cd "$(dirname "$0")"
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

SESSION="corrosive-mesh"
BIN="target/release/decentralised_agents"

if [ "${1:-}" = "kill" ]; then tmux kill-session -t "$SESSION" 2>/dev/null || true; exit 0; fi
command -v tmux >/dev/null || { echo "tmux is required (brew install tmux)"; exit 1; }
[ -x "$BIN" ] || { echo "build first:  cargo build --release"; exit 1; }

ATTACH=1
if [ "${1:-}" = "--no-attach" ]; then ATTACH=0; shift; fi
QUESTION="${1:-What is the Raft consensus algorithm, and when should I choose it over Paxos?}"

tmux kill-session -t "$SESSION" 2>/dev/null || true
tmux new-session -d -s "$SESSION" -x 240 -y 60

# Three equal rows, then split each row into two columns → a 3×2 grid.
tmux split-window -v -t "$SESSION"
tmux split-window -v -t "$SESSION"
tmux select-layout -t "$SESSION" even-vertical
for p in $(tmux list-panes -t "$SESSION" -F '#{pane_id}'); do
  tmux split-window -h -t "$p"
done

# Deterministic pane order: sort by top, then left (row-major).
mapfile -t PANES < <(tmux list-panes -t "$SESSION" -F '#{pane_top} #{pane_left} #{pane_id}' \
  | sort -n -k1,1 -k2,2 | awk '{print $3}')

ROLES=(client orchestrator router qna reasoning synthesizer)
TITLES=("1 CLIENT" "2 ORCHESTRATOR" "3 ROUTER" "4 QNA" "5 REASONING" "6 SYNTHESIZER")

# Label each pane with a stable per-pane user option (@role). Unlike
# #{pane_title}, the shell's title escape can't clobber it.
tmux set -t "$SESSION" pane-border-status top
tmux set -t "$SESSION" pane-border-format " #[bold]#{@role} "
tmux set -t "$SESSION" mouse on

# Optional pacing (ms between orchestrator stages) for a readable recording.
PACE="${DEMO_PACE_MS:-0}"

for i in "${!PANES[@]}"; do
  pid="${PANES[$i]}"
  tmux set -p -t "$pid" @role "${TITLES[$i]}"
  if [ "${ROLES[$i]}" = "client" ]; then
    tmux send-keys -t "$pid" "clear; DEMO_PACE_MS=$PACE ./run_role.sh client $(printf %q "$QUESTION")" C-m
  else
    tmux send-keys -t "$pid" "clear; DEMO_PACE_MS=$PACE ./run_role.sh ${ROLES[$i]}" C-m
  fi
done

if [ "$ATTACH" = "1" ]; then
  exec tmux attach -t "$SESSION"
else
  echo "session '$SESSION' is up (detached). Attach:  tmux attach -t $SESSION"
fi
