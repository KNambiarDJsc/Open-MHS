# Evaluation plan: read-only robot observation through OpenMHS

Committed before the confirmatory run. Anything changed after that is listed under Deviations in
`RESULTS.md`.

## Question

Can an AI or software agent inspect selected robot state through OpenMHS using explicit read-only
permissions, with provenance, freshness, failure visibility and auditability, without gaining any
ability to control the robot?

## Hypothesis

On a simulated ROS 2 robot, an agent holding a scoped read credential:

- **H1 (no control).** Cannot cause any message, service call, action goal or parameter change to reach
  the robot's control surfaces through any OpenMHS entry point it or an operator can use.
- **H2 (no false currency).** Is never given a value as current when the source has stopped, died, gone
  backwards in time, or sent something invalid.
- **H3 (visibility).** Sees each such failure as a failure, within a bound set by the configuration.
- **H4 (recovery).** Gets valid observations again after the source returns, with no restart of OpenMHS.
- **H5 (accountability).** Leaves an audit record for every read, denied read and denied command, and the
  chain verifies.

## Scope

Simulated ROS 2 only. ROS 2 Jazzy, `rclpy`, the default RMW, one Linux host, one clock, localhost DDS
discovery, a domain of its own. The simulated robot holds fixed values and has no physics. The agent is
scripted.

## Exclusions

Physical robots, real networks, packet loss, DDS security, multiple hosts, `use_sim_time`, `rclcpp`
publishers, other RMWs, real controllers, high-rate topics (nothing above 20 Hz), TF, any language
model, and any claim about safety. This is not a safety system.

## Scenarios

| Id | Scenario | Done by | Expected |
|---|---|---|---|
| A | Normal operation | 20 rounds of all 13 granted capabilities through MCP | All succeed, usable, `source: simulator`, each equal to the fixture |
| B | One topic stops (`/odom`) | Fixture stops it; poll every 20 ms; N cycles | Reads of `/odom` capabilities fail `adapter.disconnected`; `/joint_states` unaffected |
| C | Node dies | SIGKILL; poll; N cycles | Every capability fails; no value; device availability changes |
| D | Reconnect | Start the node again; poll; N cycles | Reads usable again, values equal to the fixture; gateway not restarted |
| E | Ungranted read | Read `diagnostics.driver.message`, an unknown capability, another device | Denied; audited |
| F | Attempted control | See the matrix below | All refused; zero at every control surface |
| G | Invalid data | Each fault in the table below | No value, or a value that is not usable; never usable and wrong |
| H | Timestamps | Late, future, zero, small and large backwards steps | See the table below |
| I | Audit | Export and verify | Counts match; `audit verify` and `verify-file` pass |
| J | Grounded answer | The scripted agent answers in normal and fault states | Zero grounding problems; values only from observations |

### F: attempted control matrix

Three credentials (scoped agent, unscoped agent decided by policy alone, operator), each trying: a write to a
real read-only capability, an invoke of a real read-only capability, and a write and an invoke of a control-like
capability that does not exist (`base.cmd_vel`, `joint.elbow.move`). Then:

- The operator grants itself everything and tries again. A grant cannot create an operation the descriptor does
  not declare, so this must still be refused.
- The agent submits a plan containing a write.
- A descriptor that declares `write`, with a manifest widened to allow it, must stop the adapter from starting.
- The ROS graph of the adapter's node may hold subscriptions and the framework's `/parameter_events` publisher,
  and nothing else.

**Positive control:** after the zero count is recorded, a separate ROS node pokes each control surface directly,
and each must count. Without it, a zero could be a broken counter.

### G: invalid data

| Fault | Capability | Expected |
|---|---|---|
| `nan_joint` | `joint.elbow.position` | `adapter.device_failure`, no value |
| `inf_odom` | `base.position.x` | `adapter.device_failure`, no value |
| `nan_battery_temperature` | `battery.temperature` | `adapter.device_failure`, no value |
| `range_joint` | `joint.elbow.position` | `adapter.malformed_response`, no value |
| `range_battery` | `battery.charge` | `adapter.malformed_response`, no value |
| `empty_positions` | `joint.elbow.position` | `adapter.malformed_response`, no value |
| `mismatched_arrays` | `joint.shoulder.position` | `adapter.malformed_response`, no value |
| `missing_joint` | `joint.elbow.position` | `adapter.malformed_response`, no value |
| `no_diag_status` | `diagnostics.driver.level` | `adapter.malformed_response`, no value |
| `reordered_joints` | `joint.shoulder.position` | The shoulder's own value, 0.5 rad, usable |
| `diag_error` | `diagnostics.driver.level` | Value 2, usable (a real report, not a failure) |

### H: timestamps

| Fault | Expected |
|---|---|
| `late_stamp` (5 s old, after fresh messages) | Refused as older; the last good message serves until `stale_after_ms`, with its true stamp; then `adapter.malformed_response`, no value |
| `late_stamp` from the first message | The adapter has nothing newer to compare with; value returned, `usable: false` from the gateway |
| `future_stamp` (1 h ahead) | Value returned, `usable: false`; normal stamps afterwards are accepted at once |
| `zero_stamp` | Usable, timestamp is when the message arrived; after a stop, not refreshed by reading |
| `regress_small` (200 ms back, alternating) | Over 200 reads, the reported time never goes backwards |
| `regress_step` (60 s back, held) | After `stale_after_ms`, `adapter.malformed_response`, no value; recovers when stamps recover |

## Process failure

`adapter_kill`: SIGKILL the adapter; reads fail, the host restarts it, reads recover. `gateway_kill`: SIGKILL the
gateway; the adapter exits without being forced.

## What counts as failure

The evaluation fails if any of these happens:

1. Any count at a control surface before the positive control.
2. A surface that does not count during the positive control.
3. In B or C, a read returning a value after the first failed read, while the source is still down.
4. In G or H, a usable value differing from what the fixture intended, or from a fault expected to give none.
5. In A or D, a value differing from the fixture, or a `source` other than `simulator`.
6. A denied read or command with no audit record, or `audit verify` or `verify-file` failing.
7. Any grounding problem in J.
8. Processes that needed force at cleanup.
9. Any control attempt that does not end refused.

## Metrics

All from the run, none estimated. With N cycles, report count, min, median, max.

| Metric | How measured |
|---|---|
| Read success rate | Scenario A, successes over attempts |
| Observation age | `age_ms` from the gateway in scenario A |
| Stale detection latency | Fixture's ack of the stop, to the first failed read, 20 ms polling |
| Last usable read after a stop | Ack of the stop, to the last read returning a usable value |
| Disconnect detection latency | SIGKILL returned, to the first failed read; and to the first availability other than `ONLINE` |
| Reconnect recovery latency | Fixture's `ready`, to the first usable read; and process start, to the first usable read |
| Topic recovery latency | Fixture's ack of the restart, to the first usable read |
| Denied reads, denied commands | Counts, and the matching audit records |
| Commands reaching a control surface | Must be zero |
| Audit verification | Result, record count |
| Clock steps | Changes above 20 ms of wall clock minus monotonic clock, sampled every 100 ms, signed |

**Expected latencies, stated in advance.** Stale detection should be about `stale_after_ms` (500 ms for `/odom`)
plus up to a message period (100 ms) and a polling interval (20 ms). Times above 1000 ms would mean the rule
does not work as documented. This is a bound on the configuration, not a performance claim.

N is 20. There is no statistical hypothesis: the fixture is deterministic apart from scheduling.

## Environment

A Linux host with ROS 2 Jazzy at `/opt/ros/jazzy`. The run records commit, kernel, Python, RMW and date. The
confirmatory run is under WSL2 on Ubuntu 24.04, which is a virtual machine, so latencies will not carry over to
other hosts.

## Reproduce

From a clean checkout, in a shell with no ROS 2 sourced (the example sources it itself):

```sh
cargo build -p openmhs-cli
python3 examples/ros2-readonly-robot/run_evaluation.py --cycles 20 --out /tmp/robot-eval
```
