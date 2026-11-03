# Layer 1a results: the model's premises against a real rclpy action server

**Scope.** One real server (ROS 2 Jazzy, rclpy 7.1.12, rmw_fastrtps_cpp, Python 3.12, WSL2 Ubuntu 24.04,
localhost only, `result_timeout` set to 10 s explicitly), one trial per process, 20 trials per scenario.
The server logic that matters here (goal table, duplicate-id rejection, result expiry, STATUS_UNKNOWN) lives
in `rcl_action` (C), which rclcpp servers also use; this run exercised it through rclpy. It does **not**
cover rclcpp servers, other RMWs, networks that drop packets, Nav2, MoveIt or hardware. Plan and
refutation conditions: `LAYER1_PLAN.md` (committed before the runs). Data: `results/layer1a/` (run 3, the
source of every number below). Runs 1 and 2 (`results/layer1a_run1`, `results/layer1a_run2`) were
invalidated by a flaw in my rig and are kept in the record; run 2's raw client observations were lost to my
error (see Amendment 2).

## Verdicts, using the validity rule fixed before the run

All 100 first sends were confirmed (every trial valid). No harness errors.

| Premise | Result | Verdict |
|---|---|---|
| P1 duplicate goal id rejected while held; one effect | 20 of 20 | **Held** |
| P2 after result expiry the id reads UNKNOWN, is accepted again and re-executes (two effects) | 20 of 20 | **Held** |
| P3 after server kill and restart: old id reads UNKNOWN; resend accepted; two effects | 19 of 20 agree | **Contradicted in 1 trial** |
| P4 kill before the effect: old id reads UNKNOWN; resend accepted; one effect | 19 of 20 agree | **Contradicted in 1 trial** |
| P5 same-client resend: client exception recorded, server executes it (two effects) | 20 of 20 | **Held** |

## What the contradictions are

In all three kill scenarios together (60 trials) the query for the old goal id to the restarted server
returned `UNKNOWN` in 56 and **got no answer within 8 s** in 4 (6.7 %, exact 95 % interval 1.9 % to 16.2 %).
It never returned any other status. In those four trials the server had restarted (its log shows
`server_up` 3 to 4 s into the trial) and the query was sent right after the restart. The cause is not
established. A plausible but unverified mechanism: the request was written while the client still matched
the dead server's endpoints and before it had discovered the new ones, and requests are volatile, so it was
lost. The model assumed a restarted server answers `UNKNOWN` promptly; the real behaviour adds a third
case, "no answer", which a client must treat as a further form of uncertainty. A single query was made, so
whether a second query would have succeeded is untested.

## What held, and how firmly

- **The same goal id was accepted again by the restarted server in all 60 kill trials** (the effect was
  re-executed whenever it had already happened). Exact 95 % lower limit 94.0 %.
- **Effect counts matched the prediction in all 60 kill trials** (two after the effect, one before it) and
  in all 20 expiry trials. The effect counts come only from the server's own ledger.
- **After expiry the id read `UNKNOWN` in 20 of 20 trials**, and the same id was accepted again and ran a
  second time (two effects in 20 of 20).
- **rclpy client behaviour not in the model:** resending the same id through the same client object
  raised `RuntimeError: Two goals were accepted with the same ID` in 20 of 20 trials. The server had
  already accepted and executed the goal (two effects in 20 of 20) while the client's executor thread
  was interrupted and the client saw a timeout. A duplicate effect can therefore occur with the client
  seeing an error.

## What this changes

1. **The Layer 0 premises about server semantics are supported by a real implementation**, which makes the
   Layer 0 results about duplicates and holding more credible. It does not make them measurements of ROS 2.
2. **A "no answer" mode after restart is missing from the model.** Layer 0 should gain an F3 variant in
   which the first query after restart is lost, and Layer 1b should test whether a second query succeeds.
3. **The 10 s expiry matters by implementation.** rcl_action defaults to 10 s (rclcpp servers inherit it); the
   rclpy `ActionServer` defaults to 900 s (confirmed by `inspect` on the installed package). A Python server
   with defaults would almost never produce the F4 situation; a C++ server such as Nav2's would.
4. **What remains untested:** natural message loss (F1, F2), the full arm matrix on real rclpy (Layer 1b),
   and anything involving OpenMHS (arm D).

## Rig caveat

A long-lived rclpy client that had run about 26 trials against many short-lived servers stopped receiving
goal responses for 16 trials and then recovered (runs 1 and 2, and a diagnostic of 45 `dup_held` trials).
This was treated as a rig defect and fixed with process-per-trial isolation. The reason is unknown and it
may be a ROS 2 or Fast DDS behaviour worth a separate look, but nothing here establishes that.

## Reproduce

Inside the ROS 2 Jazzy environment (see `layer1/README.md`):
`python3 layer1/probe.py --n 20 --out results/layer1a` then `python3 -I layer1/analyze.py results/layer1a`.
A full run takes about 25 minutes.
