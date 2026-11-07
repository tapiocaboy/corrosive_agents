# VHS tape — records the six-pane split-terminal mesh demo as mp4 + gif.
#   vhs demo_mesh.tape
# Produces recordings/decentralised_mesh_6pane.{mp4,gif}

Output recordings/decentralised_mesh_6pane.mp4
Output recordings/decentralised_mesh_6pane.gif

Set Shell "bash"
Set FontSize 11
Set Width 1840
Set Height 1150
Set Padding 8
Set Margin 8
Set MarginFill "#11111b"
Set Theme "tokyonight"

# Set the stage off-camera.
Hide
Type "cd /Users/kalanamith/Desktop/personal/corrosive_agents/usecases/decentralised_agents && clear"
Enter
Show

Sleep 700ms
Type "DEMO_PACE_MS=1800 ./demo_mesh.sh 'What is a Merkle tree and why is it useful?'"
Sleep 800ms
Enter

# Six libp2p nodes boot, create keys, discover peers, pass signed messages,
# call the LLM (paced for readability), and return the answer to the client.
Sleep 60s
