# Read-only robot observation through OpenMHS

**Everything here is simulated. No physical robot has been used. This is not a safety system.**

A software agent inspects a robot's state over ROS 2 through OpenMHS, with explicit read-only permissions. It
gets joint positions, odometry, battery and diagnostics, each with a unit, timestamp, age, quality and source.
When a topic stops, a node dies, or a value is invalid, it gets a failure, not an old number. It has no way to move
the robot, and the example shows that from the ROS graph as well as from the gateway's refusals.

```
simulated robot  --DDS-->  ros2_adapter.py  --stdio-->  OpenMHS gateway  --MCP-->  scripted agent
 (own process)             (subscribes only)            scopes, grants,            (answers cite observations)
                                                        freshness, audit
```

This is the existing ROS 2 adapter and the existing gateway. There is no second permission system.

## What this demonstrates

- **Controlled read-only observation.** The agent's credential names exactly which capabilities it may read.
- **Scoped access.** A capability outside its scope is refused and audited, including the driver's free-text
  message, which is deliberately not granted because device text is an injection surface.
- **No control path.** See below.
- **Provenance and freshness.** Every observation says where it came from and how old it is. "Current" means the
  gateway said `usable`; the agent adds no rule of its own.
- **Explicit failure.** Silence, death, NaN, out-of-range values, missing joints, array mismatches, and stamps
  going backwards each give a failure or a flag, and no invented value.
- **Auditability.** Reads, failed reads, denied reads and denied commands are in a hash-chained log that
  verifies, including offline. Two kinds of early refusal are the exception (see "Known limits").

## What this does NOT demonstrate

Be clear about these before relying on anything here.

- **No physical robot validation.** Nothing has run on hardware, and passing against a simulator says the contract
  and the adapter agree with each other, not that any robot works.
- **No robot control.** It cannot command anything. That is the point, and it also means nothing here tests control.
- **No motion safety.** This is not a safety system and is not in any control loop. Emergency stops, interlocks and
  controllers keep final authority.
- **No autonomous manipulation** and no planning. The agent is a script and has no model.
- **No production deployment.** OpenMHS is pre-alpha, and it has not been security audited.
- **No network security for remote deployment.** Everything is one host, localhost discovery, one clock. DDS is
  not secured here, so anything on the same domain could publish false state. OpenMHS cannot tell.
- **No safety certification.**
- **No real-world reliability.** The simulated robot has fixed values, no physics, and no real network, load or
  timing. See [RESULTS.md](RESULTS.md) for what was measured and where.

## Run it

You need Linux with ROS 2 Jazzy at `/opt/ros/jazzy`, Rust, and Python 3.10 or later. On Windows, run all of it
inside WSL: a gateway on Windows and ROS 2 in WSL have two clocks that disagree by more than the freshness limit
([why](DESIGN.md#4-what-the-baseline-and-the-first-runs-changed)). Use a shell that has not sourced ROS 2; the
example does that itself.

```sh
cargo build -p openmhs-cli

# The demo, about 30 seconds. Sample output: DEMO_TRANSCRIPT.txt
python3 examples/ros2-readonly-robot/run_demo.py

# The tests. The ROS 2 group is skipped, with a reason, if ROS 2 is not there
python3 -m unittest discover -s examples/ros2-readonly-robot -p "test_*.py" -v

# The evaluation, about 3 minutes
python3 examples/ros2-readonly-robot/run_evaluation.py --cycles 20 --out /tmp/robot-eval
```

The evaluation currently exits 1. That is one failure condition it was written to catch, and it is real: two
kinds of refused attempt leave no audit record. See "Known limits" and [RESULTS.md](RESULTS.md).

ROS 2 packages used: `ros-jazzy-ros-base` only. Set `OPENMHS_BIN` or `CARGO_TARGET_DIR` if the binary is elsewhere.

## What the robot looks like to the agent

| Capabilities | From | Note |
|---|---|---|
| `joint.{shoulder,elbow,wrist}.position` | `/joint_states` | Found by name, not array position |
| `base.position.{x,y}`, `base.orientation.{z,w}`, `base.frame`, `base.child_frame` | `/odom` | Odometry in its frame. Not a position on any map |
| `battery.{voltage,charge,temperature}` | `/battery_state` | NaN means unmeasured |
| `diagnostics.driver.level` | `/diagnostics` | 0 OK, 1 WARN, 2 ERROR, 3 STALE; a report, not a failure |
| `diagnostics.driver.message` | `/diagnostics` | Declared, and **not granted** to the agent |

The contract is [robot.manifest.json](robot.manifest.json). `/tf` is published by the robot and deliberately not
mapped, because TF has no safe flat form ([DESIGN.md](DESIGN.md)).

## Why the agent cannot control the robot

Instructions in a prompt can be argued with. These cannot, because they hold whatever the agent says:

1. **The adapter has no way to.** `ros2_adapter.py` creates subscriptions only. A test parses its source for
   publishers, service clients and action clients outside the conformance-only simulator class.
2. **The ROS graph agrees.** Inspected from a separate node, the adapter's node has subscriptions to the four declared
   topics, and `/parameter_events`, a publisher that rclpy itself creates. No services, no clients, no actions.
3. **The descriptor declares no command.** Even the operator, after granting itself everything, gets
   `command.operation.unsupported`. A grant cannot create an operation that does not exist.
4. **The adapter refuses a descriptor that declares one**, so the gateway will not start with it, and the manifest
   allows `read` only.
5. **The credential and the policy also refuse**: `auth.scope.denied` for the scoped agent, `policy.denied.explicit`
   for an agent with no scope limit.

The simulated robot hosts real control surfaces (`/cmd_vel`, `/joint_trajectory`, `/robot/set_mode`, an action
server, and a parameter), each of which counts what reaches it. The count stays at zero through all of this. Then a
separate ROS node, outside OpenMHS, pokes each one directly and each counts, so the zero is not a broken counter.

**What this does not cover.** It says nothing about other software on the ROS graph. Anything else on that domain can
publish to `/cmd_vel` whatever OpenMHS does, as the positive control shows. Connecting this to a real robot needs the
operator's approval, its own review, and ROS-level access control such as SROS 2, which is untested here.

## How a failure looks

| What happens | What the agent gets |
|---|---|
| A topic stops | `adapter.disconnected` after its `stale_after_ms`; other topics unaffected |
| The node dies | The same on every topic, each at its own limit; device availability changes |
| The node returns | Reads resume with no change to OpenMHS |
| NaN or infinity | `adapter.device_failure`, no value |
| Outside the declared range | `adapter.malformed_response`, no value |
| Joint missing, arrays mismatched | `adapter.malformed_response`, no value |
| Array order changes | The right joint's value |
| Stamp from the future, or old from the very start | The value with `usable: false` from the gateway |
| Stamps going backwards | Older messages are not used. If they persist, `adapter.malformed_response`, naming the reason |
| Driver reports ERROR | Value 2, usable: that is a true report |

## Known limits

- **Two refusals are not audited.** An MCP `submit_command` to a capability that does not exist is refused by the
  MCP server, and a plan is refused when its lease is not allowed. Neither reaches ROS 2, and neither leaves an
  audit record, so an audit reader cannot see the attempt. Closing this means a change to the core, which this
  evaluation did not make. Commands that do reach the engine are audited, including every other denial here.
- **Clocks.** Freshness needs the robot's clock and the gateway's to agree. `use_sim_time` is not supported.
- **A clock stepping back by D blinds a topic for about D**, and a looping bag until the adapter restarts. This
  environment (WSL2) steps back about 1.7 s every 30 s, so it shows up in the results.
- **`simulated_source` is a declaration.** Nothing verifies it, so a mislabelled manifest gives false provenance.
- **Reads are separate.** Pose components may come from different messages; the agent reports the spread of their
  stamps. For a moving robot this matters more than it does here, and this fixture does not move.
- **DDS discovery and QoS mismatches** can look like silence. A reliable subscription to a best-effort publisher gets
  no data and no error; the default `sensor_data` subscription avoids that case.
- **The adapter keeps the newest message per topic.** Rates here are at most 20 Hz. High-rate topics are not tested.
- **The grounding check is lexical.** It catches invented numbers and obvious overreach, not every unfaithful answer.
- **Adapter process restarts** rely on the gateway's host; that was tested, with the adapter killed by SIGKILL. The
  manifest is unsigned for this throwaway gateway (`--allow-unsigned-adapters`), which is development only.
- **The pinned container in `docs/adapters/ros2.md` is not used here**, because the fixture needs its own process.

## Files

| File | Purpose |
|---|---|
| [DESIGN.md](DESIGN.md) | The audit, external facts with sources, decisions, and what running it changed |
| [EVALUATION.md](EVALUATION.md) | Hypotheses, scenarios, expected behaviour, failure conditions, metrics |
| [RESULTS.md](RESULTS.md) | What was measured |
| [DEMO_TRANSCRIPT.txt](DEMO_TRANSCRIPT.txt) | The demo's output from a clean checkout |
| `robot.manifest.json` | Capabilities, topics, and what the adapter may do |
| `sim_robot.py` | The simulated robot: publishers, control surfaces, faults |
| `robot_lab.py` | Starts the robot, gateway, grants and credential |
| `ros_probe.py` | Inspects the ROS graph; the positive control |
| `agent.py` | The scripted agent and the grounding check |
| `run_demo.py`, `run_evaluation.py` | The demo and the evaluation |
| `test_robot_example.py` | The tests |
| `results/` | Raw evaluation output and the baseline against the unchanged adapter |
