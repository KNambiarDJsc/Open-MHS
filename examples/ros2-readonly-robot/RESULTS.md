# Results: read-only robot observation through OpenMHS

**Simulated ROS 2 only. No physical robot. This is not a safety system.** The plan is [EVALUATION.md](EVALUATION.md),
first committed before any full run (`97fd41f`). Numbers below are from the run at `5acdd25`, on a clean clone with no
uncommitted changes. Raw output is in [results/after_fix_5acdd25/](results/after_fix_5acdd25/).

## What happened

1. **The evaluation found a real gap.** The run at `6a324f4` exited 1. Two refused attempts left no audit record:
   an MCP command to a capability that does not exist, and a plan refused at its lease. Neither reached ROS 2. That run
   is kept as it was, in [results/before_fix_6a324f4/](results/before_fix_6a324f4/).
2. **Regression tests were written first,** and failed on the unchanged code
   ([before](results/regression_tests/before_fix.txt), [after](results/regression_tests/after_fix.txt)).
3. **Looking for the same fault elsewhere found worse.** The two reported cases were instances of something wider: an
   agent calling `tokens.create`, `grants.add` or `audit.export` was refused and left no record either.
4. **The shared behaviour was fixed in the gateway and the clients, not in this example**
   ([REFUSAL_AUDIT.md](../../docs/architecture/REFUSAL_AUDIT.md)).
5. **The evaluation was rerun.** It now exits 0: no failure condition was met.

## Outcome

| Hypothesis | At `6a324f4` | At `5acdd25` |
|---|---|---|
| H1 no control | Supported | **Supported.** Zero commands reached any control surface. The positive control counted all five |
| H2 no false currency | Supported in this fixture | **Supported in this fixture.** See the note on the small backwards step below |
| H3 visibility | Supported | **Supported.** Every fault was visible within the configured bound |
| H4 recovery | Supported | **Supported.** 20 of 20, with no gateway restart |
| H5 accountability | **Partly refuted** | **Supported, for what was tested.** Every refused attempt in this run has a record, and the chain verifies. What is deliberately not recorded is listed below |

### The audit gap, before and after

| Attempt | Before: records added | After: records added |
|---|---|---|
| MCP `submit_command` to `base.cmd_vel`, which does not exist | None. The MCP server refused it without calling the gateway | `command.created`, then `command.transition` to `DENIED`, code `auth.scope.denied` |
| MCP `submit_plan` containing a write | None | `request.rejected`, `plans.submit`, `lease.not_authorized`, with the principal and the device |
| An agent calling `tokens.create` (not in the original plan) | Not checked by that run; regression tests show none | `request.rejected`, `tokens.create`, `gateway.forbidden`, with who it tried to mint |

The unknown capability is now a command that was denied, because the gateway creates a receipt for one; the plan
never became anything, so it is `request.rejected`. The two are kept distinct on purpose. The agent's token appears in no
audit record, and that was checked across the whole log.

**Not recorded, on purpose:** unauthenticated requests, requests with no identifiable operation, queries, advisory
checks, and MCP argument mistakes where no operation could be formed. Near the journal's limit, refusals are refused but
not recorded. [REFUSAL_AUDIT.md](../../docs/architecture/REFUSAL_AUDIT.md) gives the reasons. A caller cannot tell from
this evaluation how complete that list is beyond the methods tested.

## Environment

| | |
|---|---|
| Commit | `5acdd25`, clean clone, gateway built from it |
| Host | WSL2 (kernel 6.6.87.2) on Windows 11, a virtual machine |
| OS, Python | Ubuntu 24.04.4, Python 3.12.3 |
| ROS 2 | Jazzy, `ros-jazzy-ros-base`, `rmw_fastrtps_cpp`, one domain, localhost discovery |
| Cycles | 20 for B, C and D. Whole run 140 s |
| Clock | One clock for robot, adapter and gateway. **No steps in this run.** The run at `6a324f4` had four, of about -1.6 s, 32 s apart. Why this virtual machine stepped then and not now is not known |

These latencies describe this virtual machine and this configuration. They are not performance claims and will not
carry over.

## Measurements

The fix did not touch the read path, so the two runs should differ only by noise and clock steps.

| Metric | n | Min | Median | Max | Median, max at `6a324f4` |
|---|---|---|---|---|---|
| Observation age, normal (ms) | 260 | 4 | 50 | 450 | 49, 485 |
| Stale detection: `/odom` stopped to first failed read (ms; limit 500) | 20 | 445 | 470 | 516 | 479, 496 |
| Last usable read after the stop (ms) | 20 | 416 | 439 | 487 | 449, 468 |
| Topic recovery: restart to usable (ms) | 20 | 47 | 80 | 119 | 70, 1121 |
| Disconnect detection: SIGKILL to first failed read (ms) | 20 | 456 | 504 | 516 | 481, 522 |
| Availability change: SIGKILL to not `ONLINE` (ms; probe every 300 ms) | 20 | 768 | 820 | 843 | 800, 836 |
| Reconnect: node `ready` to usable (ms) | 20 | 101 | 119 | 468 | 112, 478 |
| Reconnect: process start to usable (ms) | 20 | 645 | 708 | 1092 | 650, 1021 |

| Count | Result |
|---|---|
| Reads in normal operation | 260 of 260 succeeded, all usable, all `source: simulator`, none differing from the fixture |
| Values returned after the first failed read (B, C) | 0 |
| Values from any capability once all topics were silent (C) | 0 |
| Failure code in B and C | `adapter.disconnected`, 40 of 40 |
| Other topic affected during B | 0 of 20 |
| Denied reads | 4 of 4, each with a `read.denied` record |
| Commands refused through the SDK | 12 of 12, plus 2 by the self-granted operator, plus 1 through MCP, each with a denied receipt |
| Refused before any command existed | 2 of 2, each with a `request.rejected` record |
| Commands reaching a control surface | **0** |
| Positive control | All 5 surfaces reached directly by a separate ROS node, and all 5 counted |
| Audit | 2786 records. `audit verify`: hash chain verified. `verify-file` offline: valid, complete, 15 receipts, no unbound commands, no errors |
| Agent's token in any audit record | No |
| Failed reads recorded in the audit | 1125 |
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

All 11 cases as expected, each on the first read.

| Fault | Capability | Result |
|---|---|---|
| NaN elbow; infinite base x; NaN battery temperature | as named | `adapter.device_failure`, no value |
| 1000 rad elbow; charge of 7 | as named | `adapter.malformed_response`, no value |
| Empty positions; 3 names and 2 positions; elbow missing; empty diagnostics | as named | `adapter.malformed_response`, no value |
| Joint array reversed | `joint.shoulder.position` | 0.5 rad, the shoulder's own value, usable |
| Driver reports ERROR | `diagnostics.driver.level` | 2, usable: a true report |

## Timestamps

| Fault | Result |
|---|---|
| Stamps 5 s old, after fresh messages | Refused as older. The last good message served, with its true stamp, for 441 ms; first failure at 475 ms, `adapter.malformed_response`; usable 79 ms after the fault cleared |
| Stamps 5 s old from the first message | Value returned with age 5013 ms, `usable: false` |
| Stamps 1 h ahead | Value returned, age -3599943 ms, `usable: false`; usable 82 ms after the fault cleared, so one bad stamp does not hide what follows |
| Unset stamp | Age 29 ms while publishing; 346 ms after 300 ms of silence, so it is the arrival time, not the time of the read; failed at 489 ms |
| 200 ms back, alternating | 200 reads, all usable, **none going back in time** (19 of 200 before the adapter change) |
| 60 s back, held | First failure at 498 ms, naming the reason; usable 191 ms after it cleared |

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
| Adapter SIGKILL | First failed read at 10 ms, `adapter.disconnected`. The host restarted it; usable at 2792 ms. Audit still verified |
| Gateway SIGKILL | The adapter exited 153 ms later, unforced |
| Cleanup | No process needed force, in any lab in the run |

## Things that did not go as expected

1. **My stale-detection prediction was wrong in direction,** in both runs. I expected `stale_after_ms` *plus* up to a
   period and a poll. The median is under the limit, because silence is counted from the last message, which arrives
   before the stop. The range is about the limit minus a period, to the limit plus a poll. Everything stayed under the
   1000 ms bound stated in advance.
2. **The clock steps went away, and some oddities went with them.** At `6a324f4`, with four steps, one cycle had 1121 ms
   recovery, one check of the other topic failed, and `missing_joint` needed 11 reads. With no steps, recovery peaks at
   119 ms, the other topic is unaffected, and `missing_joint` needs 1. That fits the documented cost of the ordering rule,
   that a clock stepping back by D blinds a topic for about D. But the comparison is two runs, not a controlled test.
3. **Unexplained: slow reconnects.** 3 of 20 cycles (0, 7 and 12) took 435 to 468 ms from `ready`, against about 115 ms for
   the others. The earlier run had 2 of 20 the same way. With no clock steps in this run, the clock is ruled out for
   these. Discovery is a guess; the cause is not established.
4. **The adapter restart varies.** Usable again at 2.8 s here, and at 1.8 s, 2.7 s and 4.1 s in other runs. One sample per
   run, not characterized.
5. **The first version of the evaluation passed when it should not have.** The first full run (`97fd41f`) had zero
   failures, because the harness did not yet check for records on these refusals. It was reading the audit kinds in that
   run's output that exposed the gap.

## Evidence by kind

| Kind | What | Result |
|---|---|---|
| Rust, regression | `crates/openmhs-gateway/tests/refusal_audit.rs` (22), `refusal.rs` unit tests (7), MCP (4), CLI (1) | Pass. Before the fix: 13 of the 21 first written, and all 5 MCP and CLI, failed. The other 8 pin what must not change, and passed before and after |
| Rust, regression | `cargo test --workspace` on Windows | 610 passed, 0 failed |
| Unit, no ROS 2 | 16 example tests: manifest contract, agent state mapping, grounding | Pass, on Windows (Python 3.13) and Linux |
| Unit, in ROS 2 | `adapters/python/ros2/test_ros2.py` | 16 pass |
| Integration, simulated ROS 2 | 30 example tests: real gateway, real adapter, real DDS, simulated robot, `-W error::ResourceWarning` | Pass on a clean clone |
| Evaluation, simulated ROS 2 | This document | 0 failures |
| Conformance | The existing `manifest.sim.json`, with the same adapter run natively rather than in its container | Reached L1, declared L1: 29 pass, 0 fail |
| Regression | Python SDK tests (`python/tests`), on Windows | 66 pass |

## Not tested

Physical robots, or any robot's real software. Real networks, packet loss, or more than one host. DDS security
(SROS 2). `use_sim_time` or bag playback. `rclcpp` publishers, other RMWs, real controllers. Rates above 20 Hz, and any
load. The pinned container image from `docs/adapters/ros2.md`. Python 3.10, which is the CI version. A moving robot,
so torn reads across pose values were not exercised. A language model as the agent. A controlled clock step. Signed
manifests. Runs on a Linux host outside a virtual machine. Whether a client disconnecting mid-request still leaves its
refusal record: the code handles it, and no test does. The new CI step has not run on CI.

## Deviations from the plan

- After the rerun at `5acdd25`, a check as an unprivileged user failed: every earlier run in this document was as root,
  and the example used a fixed ROS log directory in `/tmp`, so a directory left by another user stopped the robot
  starting, with only "no ready within 30 s" to say so. Each run now has its own, and the robot's stderr is in the error.
  This touches no measured path. The final check below ran as an unprivileged user.
- The run at `6a324f4` is the first with the audit check, and it failed. See "What happened" above.
- Amendment 1 in the plan: after that run, an agent calling `tokens.create` was added to scenario F, and the agent's
  token in any record was added to scenario I. Condition 6 itself did not change.
- The manifest's launch command was changed to pass the adapter path as an argument, after a review for shell injection.
- Clock steps gained timestamps, so slow cycles could be compared with them.
- Scenario A originally counted a read with no value as a wrong value; it now counts it as a failed read. There were none.
- `settle` allows up to 5 s for a held fault to show, to allow for a clock step. It was added before the first full run.

## Running the tests as root

`cargo test --workspace` on Linux as root has one failure, `a_read_only_journal_sends_nothing`, because root ignores
read-only permissions. It fails the same way on `main` at `a5d8d94`, and passes as an unprivileged user. It is not caused
by this work. Run the workspace tests as an ordinary user.

## Reproduce

See [EVALUATION.md](EVALUATION.md#reproduce). Expect the same outcomes and different latencies on any other host.
