# Demo and walkthrough scripts

Everything here uses simulated devices. No hardware, no paid API, no network.

| File | What it is |
|---|---|
| [`openmhs.tape`](openmhs.tape) | A [VHS](https://github.com/charmbracelet/vhs) script that types the quickstart into a real shell: about 65 seconds. Render with `vhs demo/openmhs.tape`, or run the manual *Demo recording* workflow, which uploads a GIF and an MP4 as artifacts |
| [`../docs/assets/demo.svg`](../docs/assets/demo.svg) | The animated terminal in the README: the same commands, with the output of a real run (`python tools/record_demo.py --bin <openmhs>`). Plays once; static when animation is off |
| [`../docs/assets/demo-transcript.txt`](../docs/assets/demo-transcript.txt) | The plain-text version of that run: the fallback for readers and screen readers |
| [`../docs/assets/social-preview.png`](../docs/assets/social-preview.png) | The repository's social preview image (`python tools/make_social_preview.py`) |

The VHS recording has **not** been rendered yet: VHS is not installed on the maintainer's
Windows machine. The tape's shell commands were run by hand against the release binary,
and its running time was computed from its sleeps and typing speed, not measured.
To produce the video, run the *Demo recording* workflow (manual) and review the artifacts
before using them anywhere.

## The 60-second version

This is the README quickstart, minimal. Say it as you type it.

1. *"OpenMHS is a gateway between an AI agent and hardware. Here are three simulated devices."*
   `openmhs devices list`
2. *"Nothing is allowed until the operator allows it."* Show the grant, and that the agent
   gets a token.
3. *"The agent asks for 5 volts in one jump."* `openmhs exec ... --value 5V` -> refused: the
   supply declares a 2 V per second slew limit, and nothing was sent to the device.
4. *"A smaller step."* `--value 2V` -> `VERIFIED`: written, then read back, and it matched.
5. *"48 volts."* -> `command.value.out_of_range`. The agent cannot talk its way past it.
6. *"Everything is in a hash-chained audit log."* `openmhs audit verify`.
7. *"Pre-alpha. Simulated devices only. No hardware has been tested. Independent of
   Anthropic."*

## Narrated walkthrough (about 4 minutes)

1. **The problem (30 s).** Models guess at units and limits; permission in a prompt is a
   request; a timeout leaves you not knowing whether the device moved. Show the three
   points from the README's "Why OpenMHS".
2. **Run the guided tour (30 s).** `openmhs demo`. Do not narrate every step; point at the
   denials and at step 17 (a lost acknowledgement becomes `UNKNOWN_OUTCOME`).
3. **A gateway you control (60 s).** The 60-second version above.
4. **An agent over MCP (60 s).** `python examples/agent-loop/mock_agent.py`: the agent is
   refused, reads the reason, ramps 0 -> 2 -> 4 -> 5 V, declines 15 V, and stops to ask a
   human. Mention it needs no model and no key; show [docs/mcp.md](../docs/mcp.md) for
   Claude Code.
5. **Break it on purpose (30 s).** `openmhs sim fault sim.psu-01 apply-then-hang`, then a
   write, then `openmhs commands reconcile`. See [simulators](../docs/simulators.md).
6. **Where it stands (30 s).** The [compatibility matrix](../docs/compatibility.md): every row
   is simulated or integration-tested; none is physically validated. "Want to connect a new
   device? Build an adapter."

Do not say: "safe", "certified", "production-ready", or "compatible with MHS".
