# OpenMHS × OpenFlexure simulator quickstart

This is a simulator-only walkthrough of discovery, bounded stage requests, and the resulting
audit evidence. It does not control a physical microscope. It supports deterministic, approval-
gated, and autonomous-in-simulation runs through the same OpenMHS MCP tools.

The stage's Thing Description does **not** declare coordinate units or mechanical limits.
The demo does not call coordinates millimetres. It records integer simulator coordinates,
uses an experiment-only bound of `-100..100`, and first asks OpenMHS to check a deliberately
unit-tagged `50 mm` value. That request must be refused because the capability has no declared
unit. A human then approves the bounded simulator-coordinate request.

## Prerequisites

- Windows PowerShell, Git, Python 3.11–3.13, Rust/Cargo, and the OpenMHS repository.
- The OpenFlexure Microscope Server simulator. The upstream developer documentation describes
  simulation mode and the web-app setup step: [OpenFlexure simulator instructions](https://openflexure.gitlab.io/openflexure-microscope-server/dev/index.html).
- Loopback access to TCP port 5000. The demo refuses non-loopback URLs.

Record exact upstream commits when available. The accompanying run record captures the package
versions used for this demo; upstream default branches can move after this quickstart was written.

## Start a fresh OpenFlexure simulator

In a separate PowerShell window:

```powershell
git clone https://gitlab.com/openflexure/openflexure-microscope-server.git
Set-Location openflexure-microscope-server
python --version
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
python -m pip install -e . python-gitlab
python .\pull_webapp.py
openflexure-microscope-server --fallback -c .\ofm_config_simulation.json --host 127.0.0.1 --port 5000
```

Keep that process running. The server has sizeable binary dependencies, so ensure the selected
drive has enough free space for the environment and package cache. The first startup can fail if
the web application assets were not downloaded; `pull_webapp.py` prepares them. The simulator
uses a dummy stage and synthetic camera. The dummy stage has no optics or mechanical travel limits.

## Build and run the OpenMHS walkthrough

From the OpenMHS repository root, in another PowerShell window:

```powershell
cargo build -p openmhs-cli
$env:OPENMHS_BIN = (Resolve-Path .\target\debug\openmhs.exe).Path
$env:OPENMHS_RESEARCH_WORK = (Join-Path $PWD 'target\openflexure-research')
python research\experiments\openflexure_quickstart.py
```

The script imports the live `stage` Thing Description, probes only the stage position map,
creates an adapter manifest with one reviewed `move_x` action, and starts an isolated gateway
with an agent token. The policy allows stage reads and requires operator approval for the
single bounded invoke. The action is sent through the OpenMHS MCP server, and its expected and
observed readback is saved with the command and audit-chain report under the `normal\` or
`drop-polls\` subdirectory of `target\openflexure-research\openflexure-quickstart\`.

To exercise an accepted action whose status poll is lost, run the same walkthrough with:

```powershell
python research\experiments\openflexure_quickstart.py --drop-polls
```

The `--drop-polls` scenario requests x=-50 after the normal run has moved the simulator to
x=50. The script does not cancel or resend the action. It clears the injected poll fault,
waits for the simulator to settle, and asks OpenMHS to reconcile from readback. The resulting
x=-50 readback is distinct from the pre-command value; it is still evidence from the simulator,
not proof of physical completion or sensor truth.

## Approval-gated model run

Set `OPENAI_API_KEY` in the shell, then run the small-cost GPT-6 Luna experiment:

```powershell
$env:OPENMHS_BIN = (Resolve-Path .\target\debug\openmhs.exe).Path
$env:OPENMHS_RESEARCH_WORK = (Join-Path $PWD 'target\openflexure-openai-agent')
python research\experiments\openflexure_quickstart.py `
  --agent-model gpt-6-luna `
  --agent-base-url https://api.openai.com/v1
```

The model sees only `read_state`, `check_command`, and `submit_command`. The harness caps each
completion at 512 tokens and stops before a request if its conservative estimated cumulative
cost would exceed $0.50. It checks one integer x target in `-20..20`, strictly greater than the
fresh simulator readback, requires a matching non-denied preflight, and pauses for operator
approval before dispatch. GPT-6 Luna's Chat Completions function calling requires
`reasoning_effort=none`; the experiment sets that explicitly. A cost estimate and token usage
are saved with the trace. Do not put the API key in the repository or in a command transcript.

## Autonomous agent simulator run

The agent can also execute one simulator action without an approval pause. This mode gives its
isolated gateway an invoke grant only for `ofm.stage/move_x`; the imported action contract is
narrowed to `-2..2` simulator coordinates. A pre-send guard allows exactly one submit only when
it follows a fresh read and a non-denied preflight for the identical parameters, and the target is
exactly one coordinate above the read value. The task asks the agent to read, compute that target,
submit once, and independently read back the new state. A second or mismatched submission is held
by the harness. This experiment-only grant does not apply to any other gateway or device.

```powershell
$env:OPENMHS_BIN = (Resolve-Path .\target\debug\openmhs.exe).Path
$env:OPENMHS_RESEARCH_WORK = (Join-Path $PWD 'target\openflexure-autonomous-agent')
python research\experiments\openflexure_quickstart.py `
  --agent-model gpt-6-luna `
  --agent-base-url https://api.openai.com/v1 `
  --autonomous-simulator
```

The autonomous run reserves headroom for the earlier model run by setting its own estimated
request ceiling to $0.49. It uses the same 512-token completion cap and GPT-6 Luna's required
`reasoning_effort=none`. The live OpenFlexure Thing Description is imported into the adapter;
WoT Thing Descriptions provide machine-readable interaction affordances for properties and
actions ([W3C TD 1.1](https://www.w3.org/TR/wot-thing-description11/)). OpenFlexure documents
its motorised stage and scripted automation capabilities on its [project site](https://openflexure.org/).

## Guided natural-language mini-survey

This experiment lets the agent inspect the live Thing Description, identify its read and move
capabilities, and carry out a tiny four-leg square on the simulator. The human supplied the goal
in natural language; the agent selected the discovered `move_x` and `move_y` capability names
and targets. It began from the simulator's reported `(0, 0)` and attempted `+x, +y, -x, -y`,
reading the changed coordinate after every command.

The isolated gateway exposed device listing, description, state read, preflight and submission.
Its token could read state and invoke only `ofm.stage/move_x` and `ofm.stage/move_y`. Each
submission had to match the next exact one-coordinate step, follow an allowed preflight for the
same parameters, and stay within the experiment-only `-2..2` coordinate bound. A fresh matching
readback was required before the next step. The experiment did not ask for or use physical units;
the Thing Description does not declare any.

To reproduce, start the loopback simulator as described above, set `OPENAI_API_KEY` in the shell,
build the CLI, then run:

```powershell
$env:OPENMHS_BIN = (Resolve-Path .\target\debug\openmhs.exe).Path
$env:OPENMHS_RESEARCH_WORK = (Join-Path $PWD 'target\openflexure-guided-square')
python research\experiments\guided_openflexure_square.py
```

The run used GPT-6 Luna with `reasoning_effort=none`, 74,180 input tokens and 740 output tokens.
At the recorded standard rates, its estimated API cost was $0.007788. Including the preceding
OpenAI model runs recorded above, the estimated total was $0.0095342 against the user's $0.50
ceiling. Cost is an estimate from token counts and standard rates, not a billing statement.

| Scenario | Agent-discovered actions | Readbacks | End state | Audit |
|---|---|---|---|---|
| Guided mini-survey | `move_x(1)`, `move_y(1)`, `move_x(0)`, `move_y(0)` | 1, 1, 0, 0; each matched | `(0, 0)`, returned to start | all four `VERIFIED`; valid, 52 records |

The model was allowed to choose from the live-discovered interface, but the pre-send guard encoded
this exact four-step experiment. This is a guided tool-use demonstration on a simulator, not an
open-ended microscope operator, general reliability result, safety case, or physical validation.
The trace and audit export are in
[`results/reports/openflexure_quickstart/guided-square/`](results/reports/openflexure_quickstart/guided-square/).

## Recorded run

Both runs completed on 2026-10-30 against the OpenFlexure simulator at `127.0.0.1:5000`:

| Scenario | Requested | Final state | Readback | Audit |
|---|---:|---|---:|---|
| Normal | x=50 | `VERIFIED` | 50, matched | valid, 30 records |
| Lost poll after acceptance | x=-50 | `RECONCILED_APPLIED` | -50, matched | valid, 46 records |
| GPT-6 Luna agent | x=1 | `VERIFIED` | 1, matched | valid, 21 records |
| GPT-6 Luna autonomous simulator agent | x=0→1 | `VERIFIED` | 1, matched | valid, 18 records |

Reports and agent traces are in
[`results/reports/openflexure_quickstart/`](results/reports/openflexure_quickstart/).
The package versions and summary are in
[`run-info.json`](results/reports/openflexure_quickstart/run-info.json).
The lost-poll report includes an earlier recovery record at x=50; the x=-50 entry shows the
distinct 50-to-minus-50 readback transition.

All scenarios use the real OpenMHS MCP tools. In the approval-gated run, the model read x=0,
chose x=1, and waited for operator approval before `VERIFIED`. In the autonomous run, it read x=0,
computed x=1 for the relative goal, preflighted, submitted without a review prompt, then read x=1
and stopped. The second run's audit records allow authorization, dispatch, simulator execution,
and readback verification. It used 9,070 input tokens and 166 output tokens, estimated at
$0.00099. Both model runs together used about $0.00175 at standard rates, below the $0.50 limit.
This is a successful, tightly constrained simulator demonstration, not a general reliability,
safety, or physical-control claim. The cached local 9B model did not complete the same tool
sequence.

## Direct versus mediated execution

The existing comparison is documented in [DIRECT_VS_OPENMHS.md](DIRECT_VS_OPENMHS.md), with
the complete 225-run table and raw data in [RELIABILITY_BENCHMARK.md](RELIABILITY_BENCHMARK.md)
and [`results/reliability_benchmark.json`](results/reliability_benchmark.json). It includes
both a naive direct client and a careful direct client, so the comparison does not assume
OpenMHS always improves task completion.

## Timeout and reconciliation evidence

The one-device OpenFlexure move can be run above with `--drop-polls`. The larger two-device
workflow also includes clean, uncertain, reconciled, resumed, and compensated paths in
[`experiments/multi_device_experiment.py`](experiments/multi_device_experiment.py); it uses an
OpenFlexure simulator and a separate LabThings hotplate stand-in. Existing asynchronous
action scenarios, ground truth, and limitations are in
[ASYNC_ACTION_RELIABILITY.md](ASYNC_ACTION_RELIABILITY.md).

## Reproduction boundary

All generated reports are local evidence. No report should be described as hardware
validation, a safety claim, an endorsement, or a LabThings/OpenFlexure collaboration. The
captured OpenFlexure Thing Description fixture remains unnecessary for this live quickstart;
the script reads the running simulator instead.
