# OpenMHS handoff to Claude

- **Status date:** 2026-10-30 (Asia/Kolkata)
- **Repository:** `KNambiarDJsc/Open-MHS`
- **Current branch:** `main`
- **Current commit:** `4ee4ae33ff890d41796378cdbd2e3084667c6a13` (`Add guided OpenFlexure language experiment`)

This is the current project handoff. The older [HANDOFF.md](HANDOFF.md) is a historical snapshot;
its branch state and several open-item statements are stale. Check `git status` and `git log` before
working because local simulator files are not committed.

## 1. What OpenMHS is

OpenMHS is a pre-alpha Rust execution gateway between agents/software and device interfaces. It
validates a declared device contract, checks caller scope and policy, serializes local commands,
tracks lifecycle and uncertainty, reconciles by readback where possible, and records receipts and
audit evidence. It is not a safety system. No physical device has been validated, and passing tests
do not establish hardware compatibility or production readiness.

Keep the embedded SQLite gateway as the reference mode. The distributed architecture is a future
layer around a correct local execution kernel, not a reason to replace that kernel or require a
cloud service for a single-lab integration. See
[EXECUTION_KERNEL_V2.md](../architecture/EXECUTION_KERNEL_V2.md),
[KERNEL_HARDENING_VALIDATION.md](../architecture/KERNEL_HARDENING_VALIDATION.md), and
[DISTRIBUTED_MIGRATION_PLAN.md](../architecture/DISTRIBUTED_MIGRATION_PLAN.md).

## 2. Current state and changes already on `main`

`main` is pushed to `origin/main` at the commit above. The local checkout also has an untracked
`openflexure/` directory containing generated simulator data; it was locked during cleanup. Do not
stage or delete it as part of unrelated work. It is not in Git. The recent Webots attempt also left
an empty `D:\OpenMHS-Webots` directory outside the repository.

The following milestones are implemented to different degrees:

| Milestone | Status | Evidence and remaining work |
|---|---|---|
| **1. Correctness kernel** | **Implemented** | Agent-facing fixes are in `e629cb6`: no concrete example device ID in the plan tool description, unknown-device insight lookup is an explicit error, and `get_allowed_operations` reports the authenticated principal’s effective access with policy/descriptor digests and expiry. Command IDs are UUIDv7; canonical JSON is used for request/receipt digests. Lifecycle transitions are represented once as a table and tested exhaustively. Per-command dispatch still rechecks authorization; access discovery is advisory. |
| **2. Durable recovery** | **Core implemented; further fault/storage drills remain** | Receipt digests are bound to audit events; offline audit export/verification and checkpoint comparison exist; `recovery_windows.rs` exercises persisted states and repeated recovery without re-dispatch; native invocation references are retained; Python SDK `execute()` recovers lost submission responses by idempotency key and requires explicit reconciliation. Journal size limit/reserve exists. Remaining: broader dispatch/send crash windows, disk-full and filesystem/power-loss drills, interrupted reconciliation, storage segmentation/archive/retention, signatures and automated external anchoring. Existing checkpoints are unsigned and need independent retention. |
| **3. Resource ownership** | **Single-gateway serialization implemented; distributed fencing remains** | `5051fb5` adds per-device and topology-declared shared-resource locks through verification/reconciliation and holds related writes while outcomes are unresolved. Idempotency and conservative `UNKNOWN_OUTCOME`/reconcile behavior exist. These locks are process-local: there are no ownership epochs or controller-enforced stale-owner fences. Do not call them fencing. Failover between gateways must remain conservative until the controller can reject stale epochs. |
| **4. Distributed edge execution** | **Not implemented** | No PostgreSQL admission ledger/outbox, edge inbox/status sync, NATS JetStream or multi-site coordinator. Decide whether each is justified before adding it. Preserve offline local operation. Do not add PostgreSQL, NATS, Temporal and Kubernetes as a bundle. |
| **5. Generality** | **Partial simulator evidence** | OpenFlexure/LabThings and a separately authored simulated hotplate demonstrate two distinct device families. SCPI adapters/conformance exist, but the same fault-injection suite has not been run across OpenFlexure, SCPI and a separate robot/industrial simulator. Webots has not been installed or tested. No physical validation. See [GENERALIZATION_RESULTS.md](../../research/GENERALIZATION_RESULTS.md). |

The accepted lifecycle includes `UNKNOWN_OUTCOME` and explicit reconciliation. Preserve these
invariants: never resend after an ambiguous dispatch; never claim `not_sent` after acceptance;
require verification reads to postdate dispatch; only report what the device/interface evidence
actually establishes; keep deny-by-default policy and unresolved-resource holds.

## 3. Completed OpenFlexure agent experiment

The live OpenFlexure simulator experiment is complete and pushed. It gives a useful current demo
for people asking what OpenMHS does:

- GPT-6 Luna received a natural-language goal to inspect the stage interface, draw a small square,
  and return to the starting position.
- The agent discovered `position.x`, `position.y`, `move_x`, and `move_y` from the live Thing
  Description, then made four one-coordinate moves.
- A harness guard limited this run to the exact `+x,+y,-x,-y` route and `-2..2` simulator
  coordinates. Each move required an allowed exact preflight and fresh matching readback before
  the next one.
- All four receipts were `VERIFIED`; readbacks were `1,1,0,0`; final state returned to `(0,0)`;
  the 52-record audit chain validated. No physical microscope was accessed.
- This is a constrained simulator demonstration, not proof of open-ended agent reliability,
  safety, or physical compatibility.

Evidence and reproduction:

- [Audit report](../../research/results/reports/openflexure_quickstart/guided-square/audit-report.md)
- [Agent trace](../../research/results/reports/openflexure_quickstart/guided-square/agent-trace.json)
- [Run record](../../research/results/reports/openflexure_quickstart/guided-square/run-info.json)
- [Experiment code](../../research/experiments/guided_openflexure_square.py)
- [Quickstart and constraints](../../research/OPENFLEXURE_SIMULATOR_QUICKSTART.md)
- [Earlier agent study and interpretation](../../research/NATURAL_LANGUAGE_EXPERIMENTS.md)

The experiment commit is `4ee4ae3`; use a pinned report link when sharing externally:
<https://github.com/KNambiarDJsc/Open-MHS/blob/4ee4ae33ff890d41796378cdbd2e3084667c6a13/research/results/reports/openflexure_quickstart/guided-square/audit-report.md>.

## 4. Webots status: next experiment is not yet runnable here

Cyberbotics replied that Webots is a reasonable fit and invited questions on its Discord. The
proposed experiment is simulator-only and read-only: leave the Webots robot/controller running as
configured, let an agent inspect selected odometry and sensor telemetry through OpenMHS, then
silence a telemetry topic and verify that stale data becomes disconnected. Expose no `cmd_vel`,
actuator writes, services or action goals to the agent. The existing
[ROS 2 adapter](../../adapters/python/ros2/ros2_adapter.py) is already observation-only: it
subscribes to configured fields and refuses descriptors containing `write` or `invoke`.
Cyberbotics' [Webots ROS 2 packages](https://github.com/cyberbotics/webots_ros2) are the likely
integration path.

**No Webots experiment has run and no Webots code has been pushed.** Current blockers observed on
this machine:

- Webots is not installed. The official Windows R2025a installer is about 263 MB. A download to
  `D:\OpenMHS-Webots` stopped at 28,672 bytes; Windows BITS stayed at `Connecting` with zero
  transferred. The partial file was removed. No alternate official mirror was found.
- Docker Desktop was started and its WSL distros/processes appeared, but calls to the local Docker
  engine (`docker info` / `docker ps`) hung. WSL access from this environment returned
  `Wsl/E_ACCESSDENIED`. There is no confirmed usable ROS 2 container runtime.
- At the last check C: had about 244 MB free and D: about 71 GB free. Use D: for large downloads.

Suggested next sequence after the host has Webots and a working ROS 2 runtime:

1. Confirm the exact Webots ROS 2 demo publishes the selected topics and message types; do not
   assume topic names or array indices without inspecting the running graph.
2. Map only a small set of odometry and sensor fields into the ROS 2 manifest. Keep the existing
   observation-only adapter, expose no command topics, and use realistic freshness limits.
3. Ask the agent, in natural language, to report the selected robot state and sensor reading. Check
   that it can only call list/describe/read tools and verify source timestamps.
4. Stop one telemetry publisher. Confirm reads report disconnected/stale rather than returning an
   old value as current; restore the publisher and confirm recovery.
5. Save the trace, adapter logs, command/audit report and exact simulator/ROS versions. State clearly
   that the experiment is simulation-only.

If Webots has to be downloaded by the user in a normal browser, use the official
[R2025a release page](https://github.com/cyberbotics/webots/releases/tag/R2025a). Do not keep
repeating a stalled binary download in the same host environment. Once installed, check whether
the local `openflexure/` generated directory or Docker Desktop processes remain before cleanup.

## 5. Validation history and current gaps

`KERNEL_HARDENING_VALIDATION.md` records a comprehensive run of `cargo test --workspace` with 456
passed, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, Python tests
and the WoT importer suite passing. The later Python SDK recovery follow-up records 58 Python tests
passing, and the resource-serialization work adds its own concurrency tests.

For the newest OpenFlexure task, `guided_openflexure_square.py` compiled and the saved JSON bundle
was consistency-checked. A subsequent `python -m unittest discover -s python/tests -p test_agent.py`
ran 10 tests: 2 passed and 8 errored because this sandbox could not connect to its localhost MCP
gateway. Do not describe that run as a passing suite, and do not infer that it indicates a product
regression. Re-run the appropriate tests in an environment that permits the test gateway's local
network connections when validating new code.

The 225-run historical reliability benchmark and previous model experiments were not rerun as part
of the latest commit. The OpenFlexure report is a separate simulator run; do not conflate it with
the reliability benchmark.

## 6. Budget, credentials and external communication

- The user set a **hard cumulative OpenAI API spend cap of USD 0.50**. Recorded estimated spend for
  prior OpenAI model experiments is USD 0.0095342; no OpenAI calls were made during Webots setup.
  These are token/rate estimates, not a billing-system measurement. Track spend across all new
  calls and stop before making any request that could take the cumulative total over USD 0.50.
  Prefer local/offline checks where possible.
- An OpenAI key was pasted into chat history and used for earlier tests. Treat it as compromised:
  do not copy it from chat history, place it in files or logs, or reuse it. Rotate/revoke it before
  further API use; use a new key only from an environment variable. No key value is recorded here
  or in the repository.
- Email replies to Dr Richard Bowman and Cyberbotics were drafted, not sent. No Discord post, forum
  post, upstream issue or PR has been made. Do not contact anyone unless the user explicitly asks.
- Do not imply endorsement, official collaboration, safety certification, or physical-hardware
  validation by OpenFlexure, LabThings, Cyberbotics or any individual.

## 7. Git and collaboration preferences

The user's standing preference is **push directly to `main` when there is no conflict; do not open
a PR or create a task branch for this project**. Before future pushes, fetch `origin main`, inspect
the diff, and only fast-forward/push when there is no conflict. Never force push or discard the
untracked simulator directory. Current HEAD at handoff creation matches `origin/main`.

## 8. Recommended reading order

1. This handoff.
2. [Kernel hardening validation](../architecture/KERNEL_HARDENING_VALIDATION.md).
3. [Distributed migration plan](../architecture/DISTRIBUTED_MIGRATION_PLAN.md).
4. [Research index](../../research/README.md) and [decision memo](../../research/DECISION_MEMO.md).
5. [OpenFlexure natural-language experiment](../../research/NATURAL_LANGUAGE_EXPERIMENTS.md) and
   [quickstart](../../research/OPENFLEXURE_SIMULATOR_QUICKSTART.md).
6. [Current lifecycle spec](../../spec/lifecycle.md), [topology spec](../../spec/topology.md),
   and ADRs 0002, 0018, 0020 and 0021.

**Recommended immediate priority:** resolve Webots host prerequisites, then complete the bounded
read-only telemetry/staleness experiment. Keep it as a compatibility and agent-observation test;
do not expand to agent-driven robot motion until the read-only path and fault behavior are proven.
