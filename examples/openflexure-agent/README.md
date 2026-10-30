# Talk to the OpenFlexure simulator through OpenMHS

Ask an AI agent, in plain language, to run a small imaging experiment on the **OpenFlexure
simulator**, for example *"find where the sample is, refocus on the best tile and give me a report"*,
while OpenMHS checks, verifies and records every stage movement. Works with any MCP client: Claude
Desktop, Claude Code, Cursor and others.

**Simulator only.** This has not been run against a physical microscope. Before pointing it at real
hardware, review the movement envelope yourself; the Thing Description declares no units or limits.

## What the agent gets

Two MCP servers:

| Server | Tools | What it is for |
|---|---|---|
| `openmhs` | `list_devices`, `describe_device`, `get_allowed_operations`, `read_state`, `check_command`, `submit_command`, `reconcile_command`, … | Discovery, reads, and stage moves that are checked against the reviewed envelope and your grants, verified by reading the stage back, and recorded in a hash-chained audit trail |
| `openflexure-imaging` | `capture_image`, `survey_region`, `focus_sweep`, `session_report` | Microscope-level tools. Their stage moves also go through OpenMHS with the same agent token, so nothing bypasses the gateway. A refused, unapproved or uncertain move stops the tool and is reported; nothing is resent |

Image measurements (sample coverage, feature count, sharpness) are deterministic image statistics.
The agent sees the numbers, not the images, so it cannot claim to have seen something it was not
told.

## Five minutes, on one machine

1. Start the OpenFlexure simulator on `127.0.0.1:5000`
   ([research/OPENFLEXURE_SIMULATOR_QUICKSTART.md](../../research/OPENFLEXURE_SIMULATOR_QUICKSTART.md)).
2. Build OpenMHS: `cargo build -p openmhs-cli`.
3. Connect. Use a Python that has numpy, Pillow and scipy for `--python` (the OpenFlexure server's
   environment has them):

   ```powershell
   python examples\openflexure-agent\start.py --python C:\path\to\openflexure\.venv\Scripts\python.exe
   # --approval   every stage move waits for a human (approve with: openmhs commands approve <id>)
   # --stop       stop the gateway again
   ```

   It imports the stage, starts a gateway, grants one agent read access and bounded moves
   (x, y within ±1200 and z within ±400 simulator steps), and writes `mcp-config.json`.
4. Add the two servers to your agent: merge `mcpServers` from `mcp-config.json` into Claude
   Desktop's `claude_desktop_config.json`, or `claude mcp add-json <name> '<entry>'` for Claude Code.
   The file holds a local agent token; treat it like a password.
5. Ask. For example:

   > There's a slide on the microscope stage. Please find where the sample is: survey a 5x5 area
   > around the current position with 500-step spacing and tell me which tiles have the most sample.
   > Then refocus on the best tile and give me a session report, including whether the audit check
   > passed.

Check the plumbing with no model at all: `python examples\openflexure-agent\smoke_test.py <mcp-config.json>`.

## What a session leaves behind

`session_report` writes a Markdown report: every move with its OpenMHS command id and state, every
image with its position, measurements and sha256, an OpenMHS audit export, and the result of
verifying that export offline (`openmhs audit verify-file`). Exporting evidence is an operator
action; the report tool performs that one read-only step with the local operator credential, and the
agent never receives it.

## A recorded run (2026-10-30)

The request above, sent to GPT-6 Luna with both servers attached
([research/experiments/technician_session.py](../../research/experiments/technician_session.py)):

- The model called `survey_region` (5×5, spacing 500), then `focus_sweep` at (0, 0), then
  `session_report`. It made **no direct stage moves**.
- All 25 tiles captured. It named the top tiles (0, 0), (−500, 0) and (500, 0) with their sample
  fractions, z = 0 as the sharpest focus, and that the audit check passed. **Every number in its
  answer appears in the tools' output.**
- 34 moves, all `VERIFIED`; 30 images; offline audit verification exit 0; no unresolved outcome.
- One model session: estimated $0.0021.

Evidence: [research/results/reports/technician_session/](../../research/results/reports/technician_session/).
One run on a simulator: it shows the path works end to end, not that it is reliable across tasks,
models or real samples.

## Limits

- Images are camera reads by the imaging server, linked to OpenMHS-verified positions by time and
  position; OpenMHS does not attest image content.
- Measurement thresholds are tuned for the simulator's synthetic sample.
- `--allow-unsigned-adapters` is used for the generated adapter: development mode.
- The release binaries for v0.1.0 predate this example; build from `main`.
