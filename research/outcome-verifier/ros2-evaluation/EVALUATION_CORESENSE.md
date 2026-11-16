# Evaluation plan: real drone flights, action status against motion capture

Written and committed before the measured run. Changes afterwards go under Deviations in `RESULTS_CORESENSE.md`.

**Real hardware, recorded in October 2026, replayed offline. Not live.** Failures in these flights were **induced by the
experimenters**; none is an organically observed field failure.

## Question

When a real drone's ROS 2 action server reports `SUCCEEDED` for a takeoff, a go-to or a landing, was the drone where the
goal said it should be, according to motion capture? Does the verifier tell us more than a short script would?

## Data

The CoreSense Inspection Testbed ROSbag Dataset: three Crazyflie drones, Aerostack2, OptiTrack motion capture. Zenodo
10.5281/zenodo.23243295, CC BY 4.0, MD5-checked. Three flights in the bags used: `no_failure`, `panel_failure`,
`double_failure`. Source of everything below: `/droneN/<Behavior>Behavior/_action/status` (`GoalStatusArray`),
`/droneN/kb/add_fact` (`std_msgs/String`), `/droneN/self_localization/pose` (`PoseStamped`, frame `earth`, about 115 Hz).

**What the reference is, and is not.** `self_localization/pose` comes from the motion-capture system, which also feeds the
drone's controller. It is independent of the action's *claim* (the behavior's own success rule) but **not independent of the
sensor the behavior used**. A `VIOLATED` verdict therefore means the behavior's success rule is looser than the contract's
tolerance, not that a sensor was wrong. A `SATISFIED` verdict confirms agreement with the measured position; it does not
prove the drone was physically there beyond motion capture's own accuracy.

## Claims

Every goal that reaches `SUCCEEDED`, in all three flights, for three behaviors. The claim time is the recorder's receipt
time of the first status message showing `SUCCEEDED`.

| Behavior | Claim | Contract (fixed now) |
|---|---|---|
| `GoTo` | "the drone arrived at its waypoint" | Pose x and y each within **±0.20 m** of the waypoint's `xCoord`, `yCoord` (knowledge-base facts `wp_K`, joined to the goal by `goto_N to wp_K` at the goal's start time), frame `earth`, every sample within **±50 ms** of the claim |
| `Takeoff` | "the drone is at takeoff height" | Pose z within **±0.25 m** of **1.5 m** (the `height` argument in the mission item, read from `mission_status`) |
| `Land` | "the drone is on the ground" | Pose z **≤ 0.15 m** |

Not claims: `CANCELED` goals (nothing was claimed), `CollisionAvoidance` goals (not a position claim).

## Labels

Computed by a separate path: a direct read of the bag for the pose sample nearest the claim time (within 50 ms), box
check. Not through the importer's events, the evidence file or the verifier. `AT_TARGET`, `NOT_AT_TARGET`, or
`NO_REFERENCE` (no sample within 50 ms).

## What is compared

| | Says |
|---|---|
| Native | The action status: always "arrived" |
| Script | The nearest pose within ±50 ms, box check; "no data" if none. Ignores frame, clock, goal id and device |
| Verifier | `openmhs outcome verify` on the contract above |

## Predictions, before running

1. Most `SUCCEEDED` goals will be `SATISFIED`: the behavior and the contract both read the same pose. The count of
   `VIOLATED` is not known in advance; it depends on Aerostack2's own success tolerance, which this plan does not know.
2. Script and verifier will agree on every claim except where the single nearest sample and the ±50 ms window straddle
   a box edge (a moving drone): the verifier needs all samples in the window inside.
3. **No false `SATISFIED`** from the verifier against a label. One would be a failure of this evaluation.
4. Sensitivity to the tolerance (0.10 m and 0.30 m for `GoTo`) is reported for labels and the script only; the verifier's
   verdicts are for the fixed contract.

## Injected faults (on copies of real records; none happened in the flights)

Applied to every claim: reference in another frame (`odom`, rotated and shifted); reference on another clock (1.5 s late,
no stated uncertainty); reference removed within 1 s; claim with another goal's id. Expected from the earlier
experiments: the verifier `UNKNOWN` each time with the stated reason; the script wrong where the numbers change.

## Also measured

Byte-identical reports across two runs; verification time per claim; the share of claims by verdict per flight.

## Failure conditions

Any false `SATISFIED` from the verifier. Any non-deterministic report. Any injected-fault verdict that differs from the
expectation above.
