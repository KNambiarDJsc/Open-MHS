# Layer 1a plan: do the model's premises hold on a real rclpy action server?

Written and committed before the n = 20 run. A development smoke run (one trial per scenario, used only
to fix the rig) is the only data seen so far; its observations are not reported as results.

## What Layer 1a is

A characterisation of one real server (ROS 2 Jazzy, rclpy 7.1.12, rmw_fastrtps_cpp, Python 3.12, WSL2
Ubuntu 24.04 in an isolated distro) against the **premises** Layer 0 assumed. It is not a fault-rate
study, covers one RMW, and says nothing about rclcpp servers, Nav2, MoveIt or any other implementation.
`example_interfaces/action/Fibonacci` is used only as a carrier; `order` is the logical operation id and
the effect is a line fsync'd to a ledger file that no client reads.

## Premises tested (each with n = 20 trials)

| Premise in the model | Scenario | Model predicts | The model is wrong about this if |
|---|---|---|---|
| P1: a duplicate goal id is rejected while the goal is held | `dup_held` | second send rejected, 1 effect | the second send is accepted or the effect count is not 1 in any trial |
| P2: after the result expires, a query reads STATUS_UNKNOWN, and the same goal id is accepted again and re-executes | `expiry_10s` | late status UNKNOWN, resend accepted, 2 effects | late status is not UNKNOWN, or the resend is rejected, in any trial |
| P3: after a server kill and restart, the old goal id reads STATUS_UNKNOWN and a resent id is accepted and re-executes if the effect had happened | `kill_after_effect` | UNKNOWN, accepted, 2 effects | any other status, rejection, or an effect count other than 2 |
| P4: a kill before the effect loses the effect, and a resend produces it once | `kill_before_effect` | UNKNOWN, accepted, 1 effect | an effect count other than 1 |
| P5 (rclpy client behaviour, not in the model): a resend of the same id through the same client object fails client-side while the server still executes it | `kill_after_effect_same_client` | client exception recorded, server effect count 2 | no client exception, or an effect count other than 2 |

A trial counts as a violation of the premise if it disagrees with the prediction. A premise is reported
as "held" only if all 20 trials agree (exact 95 % lower limit 83.2 % that the true agreement rate is at
least that), and otherwise the disagreements are listed.

## Fixed settings

Goal duration 3.0 s, effect at 50 % of the duration, server `result_timeout` set to 10 s **explicitly**
(rcl_action's default, which rclcpp servers inherit; the rclpy `ActionServer` default is 900 s, a
difference found in the source and confirmed by `inspect` on the installed package). A fresh action name per
trial; the restarted server in a kill trial keeps the trial's name. Restart delay 1.5 s. SIGKILL for the
fault; SIGINT for the clean end of a trial. `ROS_LOCALHOST_ONLY=1`, `ROS_DOMAIN_ID=77`.

## Out of scope for 1a

Message-loss faults at the DDS level (F1, F2), the full arm matrix on real rclpy (Layer 1b), the
OpenMHS-governed arm D, other RMWs (Cyclone, Zenoh), rclcpp servers, and any timing statistics.

## Amendment 1 (written after run 1, before run 2)

**Run 1** (`results/layer1a_run1/`, n = 20 per scenario, 100 trials, no harness errors) gave: P1 held in
20 of 20. In 14 of 20 `expiry_10s` trials and 3 of 20 `kill_after_effect` trials the client never received a
response to its **first** send within 8 s, so the premise could not be tested in those trials. The
server-side timelines of those trials were lost because the rig kept its work files in the distro's
`/tmp`. A diagnostic rerun of `expiry_10s` alone (n = 10, in `D:\openmhs-tmp\l1diag`, not part of the
results) had every first send confirmed in under a second. The cause of the run 1 timeouts is therefore
unexplained and intermittent.

**Changes to the rig for run 2:** work files (ledger, server events) are kept next to the results, and each
observation carries the server-side event timeline for that trial.

**Validity rule, fixed before run 2:** a trial is *valid for the premise test* only if the first send
returned `accepted` within 8 s. Invalid trials are counted and reported per scenario, never discarded
silently, and never counted as agreement or disagreement. A premise is reported "held" only if every valid
trial agrees and at least 15 of 20 trials were valid. Otherwise it is reported "inconclusive". Whatever
causes unconfirmed first sends is itself a finding (a goal accepted by the server whose response never
arrives is the F1 fault occurring naturally), and the timelines will show whether the server received it.

**Run 2** goes to `results/layer1a/`. Run 1 stays in the record.
