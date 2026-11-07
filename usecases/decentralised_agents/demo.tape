# VHS tape — records the decentralised agent mesh demo as mp4 + gif.
#   vhs demo.tape
# Produces recordings/decentralised_agents_demo.{mp4,gif}

Output recordings/decentralised_agents_demo.mp4
Output recordings/decentralised_agents_demo.gif

Set Shell "bash"
Set FontSize 13
Set Width 1180
Set Height 900
Set Padding 20
Set Margin 14
Set MarginFill "#11111b"
Set Theme "tokyonight"
Set WindowBar Colorful

# Set the stage off-camera.
Hide
Type "cd /Users/kalanamith/Desktop/personal/corrosive_agents/usecases/decentralised_agents && clear"
Enter
Show

Sleep 600ms
Type "./demo.sh"
Sleep 700ms
Enter

# Let the five libp2p nodes form a mesh and run the full pipeline.
Sleep 42s
