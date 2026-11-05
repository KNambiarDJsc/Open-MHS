# Results: read-only robot observation through OpenMHS

**Simulated ROS 2 only. No physical robot. This is not a safety system.** The plan is [EVALUATION.md](EVALUATION.md),
committed before the first full run (`97fd41f`). Numbers below are from the run at `6a324f4`, on a clean clone
with no uncommitted changes. Raw output is in [results/confirmatory/](results/confirmatory/).

## Outcome

**The evaluation exits 1: one failure condition was met.** Condition 6 asks for an audit record on every denied
read or command, and two kinds of refusal have none. Nothing else on the list was met.

| Hypothesis | Result |
|---|---|
| H1 no control | **Supported.** Zero commands reached any control surface. The positive control counted all five |
| H2 no false currency | **Supported in this fixture.** See the note on the small backwards step below |
| H3 visibility | **Supported.** Every fault was visible within the configured bound |
| H4 recovery | **Supported.** 20 of 20, with no gateway restart |
| H5 accountability | **Partly refuted.** Reads, failed reads, denied reads and denied commands are recorded and verify. Two refusals made before a command exists are not recorded |

### The failure: two refusals leave no audit record

| Attempt | Refused by | Audit records added |
|---|---|---|
| MCP `submit_command` to `base.cmd_vel`, which does not exist | The MCP server, `command.capability.unknown`, before anything reaches the engine | 0 |
| MCP `submit_plan` containing a write | The engine, `lease.not_authorized`, when the plan's lease is refused | 0 |

Neither reached ROS 2. The gap is that someone reading the audit log cannot tell these were tried, and a probe for
capabilities through MCP would be invisible. The same command to `base.cmd_vel` through the SDK, by the scoped agent,
does leave a denied receipt. Closing this needs a change to the core, which this evaluation did not make. A test pins
the present behaviour, so closing it will flip that test.

## Environment

| | |
|---|---|
| Commit | `6a324f4`, clean clone, gateway built from it |
| Host | WSL2 (kernel 6.6.87.2) on Windows 11, a virtual machine |
| OS, Python | Ubuntu 24.04.4, Python 3.12.3 |
| ROS 2 | Jazzy, `ros-jazzy-ros-base`, `rmw_fastrtps_cpp`, one domain, localhost discovery |
| Cycles | 20 for B, C and D. Whole run 141 s |
| Clock | One clock for robot, adapter and gateway. It stepped back four times in the run: -1539, -1551, -1582 and -1553 ms, about 32 s apart |

These latencies describe this virtual machine and this configuration. They are not performance claims and will not
carry over.

## Measurements

| Metric | n | Min | Median | Max |
|---|---|---|---|---|
| Observation age, normal (ms) | 260 | 4 | 49 | 485 |
| Stale detection: `/odom` stopped to first failed read (ms; limit 500) | 20 | 454 | 479 | 496 |
| Last usable read after the stop (ms) | 20 | 258 | 449 | 468 |
| Topic recovery: restart to usable (ms) | 20 | 42 | 70 | 1121 |
| Disconnect detection: SIGKILL to first failed read (ms) | 20 | 455 | 481 | 522 |
| Availability change: SIGKILL to not `ONLINE` (ms; probe every 300 ms) | 20 | 765 | 800 | 836 |
| Reconnect: node `ready` to usable (ms) | 20 | 104 | 112 | 478 |
| Reconnect: process start to usable (ms) | 20 | 602 | 650 | 1021 |

| Count | Result |
|---|---|
| Reads in normal operation | 260 of 260 succeeded, all usable, all `source: simulator`, none differing from the fixture |
| Values returned after the first failed read (B, C) | 0 |
| Values from any capability once all topics were silent (C) | 0 |
| Failure code in B and C | `adapter.disconnected`, 40 of 40 |
| Denied reads | 4 of 4, each with an audit record |
| Commands refused through the SDK | 12 of 12, plus 2 by the self-granted operator, each with a denied receipt |
| Commands reaching a control surface | **0** |
| Positive control | All 5 surfaces reached directly by a separate ROS node, and all 5 counted |
| Audit | 2918 records. `audit verify`: hash chain verified. `verify-file` offline: valid, complete, 14 receipts, no unbound commands, no errors |
| Failed reads recorded in the audit | 1214 |
| Grounding problems | 0, in the normal answer and the one with a topic stopped |

## Control, in detail

| Who | `joint.elbow.position` write or invoke | `base.cmd_vel` write, `joint.elbow.move` invoke |
|---|---|---|
| Scoped agent | `auth.scope.denied` | `auth.scope.denied` |
| Agent with no scope limit (policy alone) | `policy.denied.explicit` | `command.capability.unknown` |
| Operator | `policy.denied.no_grant` | `command.capability.unknown` |
| Operator after granting itself everything | **`command.operation.unsupported`** | not repeated |

None was dispatched to the adapter. The self-granted operator is the structural result: the descriptor declares no
command, so no policy can create one. A descriptor declaring `write`, with a manifest widened to allow it, stopped the
gateway from starting: `operations ['write'] are not supported; the ROS 2 adapter only observes`.

**The adapter's node, inspected from a separate ROS node:** subscriptions to `/battery_state`, `/diagnostics`,
`/joint_states`, `/odom`; publisher `/parameter_events` (created by rclpy); no services, service clients, action
servers or action clients. Before this work it had seven services, including `set_parameters`, and `/rosout`.

## Invalid data

All 11 cases as expected.

| Fault | Capability | Result |
|---|---|---|
| NaN elbow; infinite base x; NaN battery temperature | as named | `adapter.device_failure`, no value |
| 1000 rad elbow; charge of 7 | as named | `adapter.malformed_response`, no value |
| Empty positions; 3 names and 2 positions; elbow missing; empty diagnostics | as named | `adapter.malformed_response`, no value |
| Joint array reversed | `joint.shoulder.position` | 0.5 rad, the shoulder's own value, usable |
| Driver reports ERROR | `diagnostics.driver.level` | 2, usable: a true report |

`missing_joint` needed 11 reads, about a second, to reach the expected state; the others needed 1. The harness did not
record when each case ran, so this cannot be compared with the clock steps, and the reason is not established.

## Timestamps

| Fault | Result |
|---|---|
| Stamps 5 s old, after fresh messages | Refused as older. The last good message served, with its true stamp, for 502 ms; first failure at 528 ms, `adapter.malformed_response`; usable 23 ms after the fault cleared |
| Stamps 5 s old from the first message | Value returned with age 5021 ms, `usable: false` |
| Stamps 1 h ahead | Value returned, age -3599963 ms, `usable: false`; usable 24 ms after the fault cleared, so one bad stamp does not hide what follows |
| Unset stamp | Age 19 ms while publishing; 361 ms after 300 ms of silence, so it is the arrival time, not the time of the read; failed at 512 ms |
| 200 ms back, alternating | 200 reads, all usable, **none going back in time** (19 of 200 before the change) |
| 60 s back, held | First failure at 474 ms, naming the reason; usable 51 ms after it cleared |

**A note on H2.** While stamps alternate 200 ms back, values stay usable because the newer messages keep arriving. This
fixture's values do not change, so a stale value could not be told from a fresh one by value here. The check is on the
reported time.

## The baseline, before the adapter was changed

From [results/baseline_unmodified_adapter/baseline.json](results/baseline_unmodified_adapter/baseline.json), adapter at
`a5d8d94`. These are why the adapter was changed.

| Fault | Unmodified adapter |
|---|---|
| Joint array reversed | **The wrist's 0.3 rad returned as the shoulder, `GOOD`, usable** |
| 1000 rad elbow | **1000 rad returned, `GOOD`, usable** |
| 200 ms back, alternating | 19 of 200 reads went back in time, all usable |
| `DiagnosticStatus.level` | Always `adapter.malformed_response` |

## Process failure

| Event | Result |
|---|---|
| Adapter SIGKILL | First failed read at 11 ms, `adapter.disconnected`. The host restarted it; usable at 4114 ms. Audit still verified |
| Gateway SIGKILL | The adapter exited 153 ms later, unforced |
| Cleanup | No process needed force, in any lab in the run |

## Things that did not go as expected

1. **My stale-detection prediction was wrong in direction.** I expected `stale_after_ms` *plus* up to a period and a
   poll. The median was 479 ms, under the 500 ms limit, because silence is counted from the last message, which arrives
   before the stop. The range is about the limit minus a period, to the limit plus a poll. All results stayed under
   the 1000 ms bound stated in advance.
2. **A clock step probably reached the results.** B's cycle 10 finished 1.6 s after the -1539 ms step, and in it the last
   usable read came early (258 ms) and recovery took 1121 ms. That fits the documented cost of the ordering rule, that a
   clock stepping back by D blinds a topic for about D. Separately, in 1 of B's 20 cycles the check on the other topic
   did not get a usable read; the run did not record which cycle. The coincidence is shown; a controlled step was not run.
3. **Two reconnects were slower for no recorded reason.** 406 ms and 478 ms from `ready` against a median of 112 ms, with
   no clock step within 4 s of those cycles. Discovery is a guess; the cause was not established.
4. **The adapter restart took 2.7 s in one run and 4.1 s in this one.** Only one sample per run; the spread is not
   characterized.

## Evidence by kind

| Kind | What | Result |
|---|---|---|
| Unit, no ROS 2 | 16 tests: manifest contract, agent state mapping, grounding | Pass on Windows (Python 3.13), and on Linux |
| Unit, in ROS 2 | `adapters/python/ros2/test_ros2.py`: selectors, ranges, ordering, source parse for control calls | 16 pass |
| Integration, simulated ROS 2 | 28 tests: real gateway, real adapter, real DDS, simulated robot, `-W error::ResourceWarning` | Pass on a clean clone |
| Evaluation, simulated ROS 2 | This document | 2 failure records, both condition 6 |
| Conformance | The existing `manifest.sim.json`, with the same adapter run natively rather than in its container | Reached L1, declared L1: 29 pass, 0 fail |
| Regression | Python SDK tests (`python/tests`), on Windows | 66 pass; no SDK file changed |
| Regression | `tools/gen_matrix.py --check`, `tools/check_registry.py` | Pass |
| Rust | No crate changed (`git diff main -- crates spec python` is empty), so the workspace tests were not rerun | Not run |

## Not tested

Physical robots, or any robot's real software. Real networks, packet loss, or more than one host. DDS security
(SROS 2). `use_sim_time` or bag playback. `rclcpp` publishers, other RMWs, real controllers. Rates above 20 Hz, and any
load. The pinned container image from `docs/adapters/ros2.md`. Python 3.10, which is the CI version. A moving robot,
so torn reads across pose values were not exercised. A language model as the agent. Procedures as a way in. A controlled
clock step. Signed manifests. Runs on a Linux host outside a virtual machine. The new CI step has not run on CI.

## Deviations from the plan

- The first full run (`97fd41f`) had zero failures, because the harness did not yet check for an audit record on the two
  early refusals. I noticed the gap when reading that run's audit kinds, and added the check. The numbers above are
  from the run after that. This made the outcome worse, not better.
- The manifest's launch command was changed to pass the adapter path as an argument, after a review for shell injection.
- Clock steps gained timestamps, so slow cycles could be compared with them.
- Scenario A originally counted a read with no value as a wrong value; it now counts it as a failed read. There were none.
- `settle` allows up to 5 s for a held fault to show, to allow for a clock step. It was added before the first full run.

## Reproduce

See [EVALUATION.md](EVALUATION.md#reproduce). Expect the same two failure records until the gap is closed, and different
latencies on any other host.
