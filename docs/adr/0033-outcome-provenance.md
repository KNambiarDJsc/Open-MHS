# ADR-0033: An outcome says who says so: reported, verified, or unknown, never merged

- Status: Proposed
- Date: 2026-11-15
- Builds on: [ADR-0002](0002-command-lifecycle.md), [RFC 0004](../rfcs/0004-outcome-contracts-and-evidence.md)

## Context

Open-MHS's Gazebo experiment recorded a ROS 2 lift action reporting `SUCCEEDED` 40 times while the lift reached its
target 17 times, and odometry over-reporting travel 5.4 times
(`research/outcome-verifier/ros2-evaluation/RESULTS_GAZEBO.md`). A release dashboard built on reported status would
have scored that stack as perfect.

## Decision

An event's outcome has separate fields, each with its basis:

- `outcome.reported`: what the executing system said (`succeeded`, `failed`, `aborted`, `canceled`, `timeout`,
  `unknown`), with the source (`ros2.action_status`, `sdk`, `gateway.receipt`).
- `outcome.verified`: an RFC 0004 verdict (`satisfied`, `violated`, `unknown`) with the contract id, or absent.
- `outcome.annotated`: a person's judgement, with author.

Release statistics are computed per outcome kind and say which one they use. Disagreement between reported and
verified outcomes is itself an incident trigger.

## Consequences

- The outcome verifier (RFC 0004) becomes a producer of `outcome.verified` events, not a separate product.
- Most teams will have only reported outcomes at first; the product shows that plainly instead of hiding it.
