# E. Integration design

Rule for every integration: **read what the system already produces, keep its ids, link back to it.** Nothing here
adds a component to a robot's control path.

## 1. ROS 2

### 1.1 From recordings (Stage 1, import)

`python -m openmhs.importers.rosbag RUN_DIR --mapping MAPPING.json --manifest MANIFEST.json --out events.jsonl`

Built from `research/outcome-verifier/ros2-evaluation/ros2_evidence.py` (18 tests). A mapping file says which topics
mean what, as it does today, extended with event rules:

| ROS source | Event | Join keys |
|---|---|---|
| `/<action>/_action/status` (`action_msgs/GoalStatusArray`) | `action.status` per transition; `action.started` at ACCEPTED/EXECUTING; `action.ended` at SUCCEEDED/ABORTED/CANCELED with `outcome.reported` | `goal_id` (UUID), `ext.ros2.action` |
| Action introspection topics (Kilted+, opt-in) | goal request and result payload in `ext.ros2` (result codes) | `goal_id` |
| `/diagnostics` (`diagnostic_msgs`) | `fault.raised` on transition to WARN/ERROR, `fault.cleared` on OK | robot, component = hardware_id/name |
| `ros2_medkit` fault topics/services | `fault.*` with medkit's fault code and confirmation state | fault code |
| `/rosout` (`rcl_interfaces/Log`), level ≥ ERROR | `marker` or `fault.raised` by pattern table | node name |
| A task topic the team already has (RMF task state, BT log, app topic) | `task.started`/`task.ended` via a field mapping | `task_id` field |
| `/clock` | Time placement for unstamped messages, with stated uncertainty | — |
| The bag itself | `artifact.registered` with SHA-256, time range, topics | — |

Known constraints, from the repository's own experiments: rosbag2 (Jazzy) recorded the action status topic with an
empty schema, handled by a supplied definition that is recorded in the import metadata; unstamped messages need
`/clock` or a stated receipt-time assumption; Gazebo's pose topic needed a republisher because `ros_gz_bridge` drops
names and stamps.

### 1.2 Live (Stage 2, conditional)

A read-only ROS 2 node, `openmhs_ros_observer`, that subscribes to the same topics and writes the spool. It never
creates action clients, never publishes and never calls services, the same posture as
`adapters/python/ros2/ros2_adapter.py`. Tested in the existing Jazzy container job in CI.

### 1.3 Task identity

ROS 2 has goal UUIDs but no standard task id. Three ways, in order of preference: the team's dispatcher emits
`task.*` events with the SDK; the importer maps an existing topic; the importer groups actions into synthetic tasks by
a declared rule (e.g. one `navigate_to_pose` goal is one task) and marks them `provenance: imported`, basis rule.

## 2. MCAP, Foxglove, Roboto, Rerun

- **Artifacts by reference** (RFC 0005 §4). Open-MHS stores URI, digest, time range and topics.
- **Deep links**: an artifact may carry `viewer` URL templates; the timeline renders "open at this time" links (for
  Foxglove, a data-source URL with a start time; the template is configuration because URL formats are the vendor's).
- **Foxglove / Roboto events**: both products have their own event or annotation objects. An integration may export
  Open-MHS incidents as events in those systems (so engineers see them where they look at data), and import their
  events as `annotation` or `marker` with `provenance: reported`. This is a later integration, written against their
  public APIs, not reverse-engineered.
- **No MCAP writing.** Open-MHS does not re-encode recordings.

## 3. OpenTelemetry

- **Carry ids**: the SDK reads the current W3C trace context if the OpenTelemetry API is installed and fills `trace`.
  Joins on `trace_id` are `id_match`.
- **Ingest** (Stage 2): an OTLP/HTTP logs endpoint that accepts log records with `openmhs.*` attributes as envelopes,
  so services already instrumented with OpenTelemetry can send task events without the SDK.
- **Emit**: the ops service exports its own traces and metrics with OTLP (ingest latency, queue depth, rebuild time),
  for operating Open-MHS itself.
- **Not**: storing generic spans or metrics. Prometheus/Grafana and tracing backends keep those.

## 4. Deployment metadata

| Source | How |
|---|---|
| CI/CD | One step after a build: `openmhs ops manifest push manifest.json` (or `POST /v1/deployments`). A generator script reads git SHA, image digests and model revisions from the build |
| On the robot | At software start, the SDK (or a 20-line systemd helper) reads `/etc/openmhs/manifest.json` and emits `deployment.applied` with the robot id. This is the strongest attribution: the robot says what it is running |
| OTA / fleet tools | Import their rollout logs (CSV/JSON) into `deployment.applied` events, `provenance: reported` |
| Model registries | Manifest `model` components reference immutable revisions (Hugging Face commit, registry digest); Open-MHS checks only the format |

Robots that never report a manifest are counted as `excluded: no_manifest` in comparisons, never silently attributed.

## 5. Robot learning stacks

| System | Integration |
|---|---|
| LeRobot (dataset v3) | `ext.lerobot` on episodes (`repo_id`, `revision`, `episode_index`); handoff writes a selection manifest of episodes and frame ranges tagged by intervention; a reader script turns it into a filtered dataset with LeRobot's own tools. HIL-SERL's per-transition `is_intervention` flag can be derived from Open-MHS intervention intervals, and vice versa (unverified schema; to check against the current `lerobot` source) |
| Isaac Lab-Arena, LIBERO, ManiSkill | Import evaluation result files as `evaluation.run`/`.result` with suite, task, seed, checkpoint |
| GR00T / VLA runtimes | SDK events at episode, chunk (reference) and intervention granularity; model revision in the manifest |
| OpenRAL, Strands Robots | Skill calls map to `action.*` with the skill name and version in `ext`; their task ids become `task_id` |
| robomimic, Open X-Embodiment | Export only: episode references in RLDS-compatible metadata is a later option, on request |

## 6. Fleet and diagnostics tools

- **ros2_medkit**: a source of `fault.*` (above).
- **Formant, InOrbit / OpenRobOps, Viam**: import their event and incident exports as `reported` events; export
  Open-MHS incidents to their incident APIs where they exist (InOrbit documents incident-management APIs). Open-MHS
  does not replace their dashboards.
- **Ticketing (Jira, Linear, GitHub)**: a handoff can open a ticket with the package link. Later, on request.

## 7. The Open-MHS gateway

`openmhs ops import-gateway JOURNAL` reads the audit chain (not the ephemeral state fabric) and maps: a command to an
`action.*` pair (`action_id` = command id), receipts to `outcome.reported` (`UNKNOWN_OUTCOME` → `unknown`,
`VERIFIED` → `succeeded` with `ext.gateway.verified = true`), approvals and `commands.resolve` to `intervention.*`
with category `approval`/`resolve`. The gateway's formats do not change.
