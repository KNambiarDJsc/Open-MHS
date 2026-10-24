# The OpenMHS demo (about 60 seconds), recorded with VHS (https://github.com/charmbracelet/vhs).
# It types the same commands as the README quickstart into a real shell, against a real
# gateway with simulated devices. No hardware, no network, no API key.
#
# Needs `openmhs` on PATH, plus bash and python3. Render:  vhs demo/openmhs.tape
# CI renders it on request: .github/workflows/demo.yml (manual).

Output demo/out/openmhs-demo.gif
Output demo/out/openmhs-demo.mp4

Set Shell "bash"
Set FontSize 17
Set Width 1280
Set Height 760
Set Padding 24
Set TypingSpeed 35ms
Set PlaybackSpeed 1

# Setup the viewer does not need to watch: a throwaway home, a gateway, a token.
Hide
Type "export OPENMHS_HOME=$(mktemp -d) PS1='$ ' && unset OPENMHS_TOKEN" Enter
Type "openmhs serve --sim --port 0 > /dev/null 2>&1 &" Enter
Type "sleep 3 && clear" Enter
Show

Type "# OpenMHS: connect AI agents to programmable hardware. Simulated devices only." Enter
Sleep 2s
Type "openmhs devices list" Enter
Sleep 2s

Type "# Nothing is allowed until the operator allows it. Let an agent use the power supply:" Enter
Sleep 1s
Type "openmhs grants add --id agent-psu --principal agent:demo --device sim.psu-01 --capability '*' --ops read,write --effect allow" Enter
Sleep 2s

# The agent's credential is created off screen, and exported as OPENMHS_TOKEN.
Hide
Type "export OPENMHS_TOKEN=$(openmhs tokens create agent:demo --any-scope --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[\"token\"])')" Enter
Type "clear" Enter
Show

Type "# Now we are the agent. Ask for 5 V in one jump:" Enter
Sleep 1s
Type "openmhs exec sim.psu-01 power.output.voltage.setpoint --value 5V" Enter
Sleep 5s

Type "# Refused, and nothing reached the device. A smaller step:" Enter
Sleep 1s
Type "openmhs exec sim.psu-01 power.output.voltage.setpoint --value 2V" Enter
Sleep 5s

Type "# Applied, then read back from the device to confirm. And far out of range:" Enter
Sleep 1s
Type "openmhs exec sim.psu-01 power.output.voltage.setpoint --value 48V" Enter
Sleep 5s

Type "# Every step is in a hash-chained audit log. Back to the operator:" Enter
Sleep 1s
Type "unset OPENMHS_TOKEN && openmhs audit verify" Enter
Sleep 5s

Type "# github.com/KNambiarDJsc/Open-MHS  -  independent project, pre-alpha, no hardware tested." Enter
Sleep 3s

Hide
Type "kill %1" Enter
