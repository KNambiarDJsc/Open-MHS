# RFC 0005: Deployment events: envelope, identity, manifests, artifacts and storage

- **Status:** draft
- **Author:** Open-MHS maintainers
- **Date:** 2026-11-15
- **Touches:** new schema (`openmhs.event/0.1`, `openmhs.deployment/0.1`), new store (`ops.sqlite`), tooling
- **Decisions:** [ADR-0028](../adr/0028-event-envelope.md), [ADR-0029](../adr/0029-ops-storage.md),
  [ADR-0033](../adr/0033-outcome-provenance.md)

## Summary

A small, versioned event envelope for robot task execution, faults, interventions, deployments and artifacts; a
content-addressed deployment manifest; reference-only artifacts; and the SQLite schema that stores them. Designed so
that events can arrive late, twice, out of order, from rebooted robots with skewed clocks, and still produce the same
incidents.

## Motivation

The repository has no task, robot, site, deployment, release or model identity (audit §3). Without them no question
in the product thesis can be answered. The envelope adds the minimum shared vocabulary and leaves everything else to
source-specific extensions.

## 1. The envelope

```json
{
  "schema": "openmhs.event/0.1",
  "event_id": "0193a6f2-7c1e-7a51-9b0e-3f2a7d1c9e44",
  "type": "action.ended",
  "source": {
    "id": "robot-12/nav2/bt_navigator",
    "kind": "ros2",
    "boot_id": "6c0e9f5e-2b0a-4c1f-8d39-0d5f3f0c7a11",
    "seq": 18422
  },
  "time": {
    "observed": "2026-11-14T09:31:07.412381Z",
    "clock_domain": "robot-12/chrony",
    "uncertainty_ms": 5.0,
    "monotonic_ns": 912384112233
  },
  "scope": { "tenant": "acme", "site": "warehouse-east", "robot": "robot-12", "workcell": null },
  "subject": {
    "task_id": "pick-918",
    "task_type": "pick",
    "episode_id": null,
    "action_id": "navigate-3",
    "goal_id": "5b1d8a2e-4c3f-4e1a-9d7b-2a6c8f0e1b34"
  },
  "trace": { "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736", "span_id": "00f067aa0ba902b7" },
  "deployment": { "manifest": "sha256:9f2c...e1", "rollout": "rollout-2026-11-12-east" },
  "outcome": {
    "reported": { "value": "aborted", "source": "ros2.action_status", "detail_code": "nav2.controller_failed" },
    "verified": null
  },
  "evidence": [
    { "artifact": "sha256:41ab...07", "range": ["2026-11-14T09:30:40Z", "2026-11-14T09:31:10Z"],
      "topics": ["/navigate_to_pose/_action/status", "/amcl_pose", "/scan"] }
  ],
  "relations": [ { "type": "part_of", "event_id": "0193a6f1-0000-7a51-9b0e-3f2a7d1c0001" } ],
  "provenance": "observed",
  "attributes": { "component": "navigation", "severity": null },
  "ext": { "ros2": { "action": "/navigate_to_pose", "status_code": 6, "result_code": 103 } }
}
```

### 1.1 Fields

| Field | Required | Meaning |
|---|---|---|
| `schema` | yes | `openmhs.event/<major>.<minor>` |
| `event_id` | yes | Producer-assigned UUID (v7 preferred). The idempotency key within a tenant |
| `type` | yes | From the registry in §2. Unknown types are stored and ignored by rules |
| `source.id` | yes | Stable name of the emitting component, unique within a tenant |
| `source.kind` | yes | `ros2`, `sdk`, `gateway`, `import`, `operator`, `ci`, `sim`, `eval`, `verifier` |
| `source.boot_id` | no | Changes at every process or robot restart; with `seq`, orders events within one run |
| `source.seq` | no | Monotonic per (`source.id`, `boot_id`); gaps reveal loss |
| `time.observed` | yes* | When it happened, by `clock_domain`. *Absent only with `provenance: imported` and a reason in `ext` |
| `time.clock_domain` | yes | Names the clock. Same rule as RFC 0004: equal domains compare directly; different domains need stated uncertainty on both |
| `time.uncertainty_ms` | no | Absent means unknown, not zero |
| `time.monotonic_ns` | no | Source monotonic clock, for ordering within a boot |
| `time.ingested` | set by store | Never supplied by producers |
| `scope.*` | tenant, robot required | `robot` is the **logical** robot id (a role that can move to new hardware); hardware identity is in the manifest |
| `subject.*` | no | Any of task, episode, action, goal ids. Absent ids are absent, never guessed |
| `subject.task_type` | no | The producer's name for the kind of task (`pick`, `dock`, `navigate`). Release statistics group by it; without it, attempts fall in one `untyped` group |
| `subject.scenario_id` | no | What is attempted under which conditions (a task-list entry, an evaluation seed, a fixture). Attempts that share one are **replicates**: release comparisons count scenarios, not attempts, when attempts repeat a scenario (ADR-0032). Added in 0.1 during the first slice |
| `cost.wall_s` | no | Wall-clock seconds the executing system spent on the task, on any event of the task (the largest is used). Distinct from `time`, which may be simulated, replayed or imported: a simulation that runs 300 steps at 10 fps records 30 s of its own time and may have spent 800 s computing them. Non-negative number; anything else is refused. Comparable between releases only on the same hardware and load. Added in 0.1 during the policy demo |
| `trace` | no | W3C trace context when the producer has one |
| `deployment.manifest` | no | Digest of the manifest active at `time.observed`, if the producer knows it. Otherwise derived from rollouts (§3) and marked inferred |
| `outcome.reported`, `.verified`, `.annotated` | no | ADR-0033 |
| `evidence[]` | no | Artifact references with time range and topics/streams |
| `relations[]` | no | Typed edges (§1.3) |
| `provenance` | yes | `observed` (measured or emitted by the system itself), `reported` (relayed by another system), `imported` (reconstructed from a recording), `operator`, `generated` (by a model) |
| `attributes` | no | Small normalized set: `component`, `severity` (`info`, `warning`, `error`, `critical`), `category`, `code` |
| `ext.<namespace>` | no | Verbatim source data. Never interpreted by core rules |

### 1.2 Size limits

An event is at most 64 KiB serialized; `ext` at most 32 KiB. Larger payloads become artifacts. These limits protect
the edge buffer and the store, not a protocol constraint.

### 1.3 Relations

`part_of` (action in task, task in mission), `follows` (sequence within a source when `seq` is unavailable),
`retry_of`, `responds_to` (an intervention responding to a fault), `derived_from` (a verified outcome derived from
events or artifacts), `same_as` (two sources reporting one occurrence). No causal relation exists in the schema
(ADR-0030).

## 2. Event types (v0.1)

| Type | Subject | Notes |
|---|---|---|
| `task.started`, `task.ended` | `task_id` | `task.ended` carries `outcome`. A task without `ended` is open, then `unknown` after a timeout rule |
| `action.started`, `action.ended` | `action_id`, optional `goal_id` | ROS 2 actions, skill calls, policy rollouts |
| `action.status` | `goal_id` | Raw status transitions (ROS `GoalStatus`), kept for evidence |
| `episode.started`, `episode.ended` | `episode_id` | Learning episodes; may nest tasks or be nested |
| `policy.chunk` | `episode_id`, `action_id` | Reference to an action chunk (index, length, checkpoint); payload stays in the dataset |
| `fault.raised`, `fault.cleared` | optional task/action | From diagnostics, `ros2_medkit`, logs, SDK |
| `intervention.started`, `intervention.ended` | task or episode | `attributes.category`: `takeover`, `assist`, `reset`, `abort`, `approval`; operator kind: `remote`, `onsite`, `automatic` |
| `deployment.applied`, `deployment.failed` | robot | Robot started running a manifest (§3) |
| `artifact.registered` | none | A recording, video or dataset slice became available |
| `outcome.verified` | task or action | An RFC 0004 report, `derived_from` its inputs |
| `annotation` | any event or incident | Human or generated text, with author and provenance |
| `marker` | any | Free-form timeline marker |
| `evaluation.run`, `evaluation.result` | `episode_id` | Simulated or real evaluation, by reference (suite, task, seed) |

## 3. Deployment manifests and rollouts

A manifest describes *what* runs; a rollout says *where and since when*.

```json
{
  "schema": "openmhs.deployment/0.1",
  "components": [
    { "kind": "software", "name": "pick-stack", "version": "2.14.0", "vcs": "git", "revision": "3f9c2e1",
      "image": "registry.example/pick@sha256:7d1e...", "config_digest": "sha256:aa01..." },
    { "kind": "model", "name": "grasp-policy", "revision": "hf://acme/grasp-act@a1b2c3d", "framework": "lerobot",
      "checkpoint_digest": "sha256:5e7f..." },
    { "kind": "firmware", "name": "arm-controller", "version": "1.8.2" },
    { "kind": "hardware", "name": "arm", "revision": "B2", "serial": "SN-44821" },
    { "kind": "calibration", "name": "wrist-camera", "id": "cal-2026-11-02-a" }
  ],
  "labels": { "release": "2026.11.2", "channel": "canary" }
}
```

- The manifest's identity is the SHA-256 of its canonical JSON (sorted keys, no insignificant whitespace). Two
  identical manifests are one record. Model revisions must be immutable (a commit or digest, not a tag).
- Producers in other languages compute the same digest only if they write numbers the same way. Prefer strings and
  integers in manifests; a float must be written in its shortest round-trip form (`0.01`, `0.0`). The store recomputes
  every digest; an event that references a digest the store does not hold is attributed by rollout or excluded, never
  guessed. The slice's acceptance test checks Python-written references against Rust-computed digests.
- `deployment.applied` events (robot, manifest digest, time) build **rollout intervals** per robot. An event without
  `deployment.manifest` inherits the interval covering its time, with `basis: rollout_interval` and the interval's
  uncertainty. Two overlapping intervals for one robot are a recorded conflict, not a silent choice.
- "What changed" is a component-wise diff between two manifests.

## 4. Artifacts

```json
{ "artifact_id": "sha256:41ab...07", "uri": "s3://acme-logs/robot-12/2026-11-14/run_0931.mcap",
  "media_type": "application/x-mcap", "bytes": 734003200, "time_range": ["...", "..."],
  "robot": "robot-12", "streams": ["/scan", "/amcl_pose"], "viewer": "https://app.foxglove.dev/..." }
```

The artifact id is the content SHA-256 when known, else a UUID with `content_verified: false`. Open-MHS never fetches
an artifact during ingestion. Access is the storage owner's (signed URLs at Stage 2).

## 5. Identity and ordering rules

1. **Duplicates**: same tenant and `event_id` with identical canonical content → `duplicate`, ignored. Same id,
   different content → `conflict` row kept beside the first, both visible.
2. **Order within a source run**: (`boot_id`, `seq`) if present, else `monotonic_ns`, else `time.observed`.
3. **Order across sources**: by `time.observed` only when clock domains match; otherwise by time with the combined
   uncertainty, and ties within uncertainty are shown as concurrent, not ordered.
4. **Reboots**: a new `boot_id` starts a new run; sequence gaps and resets are reported per run. Open tasks of a run
   that ended are closed as `unknown` (reason `source_restarted`), never as failed.
5. **Clock skew**: at ingest, `time.ingested - time.observed` per source is recorded; a negative or implausible gap
   widens that source's effective uncertainty and is shown on the timeline. Open-MHS does not rewrite observed times.
6. **Missing ids**: an event without `task_id` can join a task only by a window rule (ADR-0030), and says so.
7. **Robot reassignment**: the logical `robot` id persists across hardware swaps; the manifest's `hardware` component
   records the physical unit. Statistics can group by either.
8. **Concurrent tasks** on one robot are allowed; window joins then become ambiguous and are marked so.

## 6. Storage schema (`ops.sqlite`, Stage 1)

```sql
CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL, checksum TEXT NOT NULL);

CREATE TABLE events (
  tenant TEXT NOT NULL, event_id TEXT NOT NULL, type TEXT NOT NULL,
  source_id TEXT NOT NULL, boot_id TEXT, seq INTEGER,
  observed_ns INTEGER, clock_domain TEXT NOT NULL, uncertainty_ms REAL, ingested_ns INTEGER NOT NULL,
  site TEXT, robot TEXT NOT NULL, task_id TEXT, episode_id TEXT, action_id TEXT, goal_id TEXT, trace_id TEXT,
  manifest TEXT, manifest_basis TEXT,            -- 'event' | 'rollout_interval' | NULL
  reported TEXT, verified TEXT, severity TEXT, category TEXT, code TEXT, component TEXT,
  provenance TEXT NOT NULL, content_hash TEXT NOT NULL, body TEXT NOT NULL,   -- canonical JSON
  PRIMARY KEY (tenant, event_id)
) WITHOUT ROWID;
CREATE INDEX events_robot_time ON events(tenant, robot, observed_ns);
CREATE INDEX events_task ON events(tenant, task_id) WHERE task_id IS NOT NULL;
CREATE INDEX events_goal ON events(tenant, goal_id) WHERE goal_id IS NOT NULL;
CREATE INDEX events_type_time ON events(tenant, type, observed_ns);
CREATE INDEX events_manifest ON events(tenant, manifest);

CREATE TABLE event_conflicts (tenant TEXT, event_id TEXT, content_hash TEXT, body TEXT, ingested_ns INTEGER,
  PRIMARY KEY (tenant, event_id, content_hash));
CREATE TABLE manifests (digest TEXT PRIMARY KEY, body TEXT NOT NULL, first_seen_ns INTEGER NOT NULL);
CREATE TABLE rollouts (tenant TEXT, robot TEXT, manifest TEXT, from_ns INTEGER, to_ns INTEGER, source_event TEXT,
  PRIMARY KEY (tenant, robot, from_ns));
CREATE TABLE artifacts (tenant TEXT, artifact_id TEXT, uri TEXT NOT NULL, media_type TEXT, bytes INTEGER,
  start_ns INTEGER, end_ns INTEGER, robot TEXT, body TEXT NOT NULL, PRIMARY KEY (tenant, artifact_id));

-- Derived, rebuildable from events (versioned by rule set):
CREATE TABLE task_attempts (tenant TEXT, task_id TEXT, robot TEXT, task_type TEXT, scenario_id TEXT, wall_s REAL, manifest TEXT,
  started_ns INTEGER, ended_ns INTEGER, reported TEXT, verified TEXT, interventions INTEGER,
  rules_version TEXT NOT NULL, PRIMARY KEY (tenant, task_id, robot, started_ns));
CREATE TABLE incidents (tenant TEXT, incident_id TEXT, rules_version TEXT NOT NULL, fingerprint TEXT,
  robot TEXT, task_id TEXT, task_type TEXT, opened_ns INTEGER, closed_ns INTEGER, triggers TEXT,
  reported TEXT, verified TEXT, manifest TEXT, PRIMARY KEY (tenant, incident_id));
-- Human state, never rebuilt:
CREATE TABLE incident_state (tenant TEXT, incident_id TEXT, owner TEXT, status TEXT, updated_ns INTEGER,
  PRIMARY KEY (tenant, incident_id));
CREATE TABLE incident_members (tenant TEXT, incident_id TEXT, event_id TEXT, basis TEXT NOT NULL,
  detail TEXT, PRIMARY KEY (tenant, incident_id, event_id));
CREATE TABLE annotations (tenant TEXT, annotation_id TEXT, target TEXT NOT NULL, author TEXT NOT NULL,
  provenance TEXT NOT NULL, kind TEXT, body TEXT NOT NULL, created_ns INTEGER NOT NULL,
  PRIMARY KEY (tenant, annotation_id));
```

- Incident ids are stable across rebuilds: a hash of (tenant, rules version, robot, task id). The implemented schema is
  `crates/openmhs-ops/src/migrations/0001_init.sql`; it is authoritative where this sketch differs.
- Window joins (rules `corr-1`) compare times only on the task's own clock domain, and skip an event whose
  `attributes.component` differs from the task's when both are given.
  Human state (owner, status, annotations) is keyed by incident id and survives a rebuild; if a rebuild no longer
  produces an incident that has human state, it is kept as `orphaned` and shown, never deleted.
- Retention (defaults, configurable): events 180 days, derived tables rebuilt as needed, annotations and incidents
  with human state kept until deleted by a person, conflicts kept with their events. Retention deletes in whole days
  and records what it deleted.
- Backup: the SQLite online backup API to a file; restore is a file copy, followed by a rebuild of derived tables.

## 7. Compatibility

- Within `0.x`: fields are only added; meanings never change; consumers ignore unknown fields and unknown event types.
- A major version comes with a converter and a period in which the store accepts both.
- The JSON Schema is generated from `openmhs-events` and checked in under `spec/schemas/`; CI fails if they diverge
  (the existing `tools/check_schemas.py` pattern).
- The gateway's audit kinds map to events through an exporter; the gateway's own formats do not change.

## Safety and security

Observation only (ADR-0026): nothing here can command a device. New risks: personal data in operator annotations and
in artifact URIs (site names, people on video); cross-tenant leakage through shared stores; spoofed events from a
compromised robot token. Mitigations are in
[06-scale-reliability-security.md](../architecture/deployment-intelligence/06-scale-reliability-security.md).
`UNKNOWN_OUTCOME` handling in the gateway is unchanged; its exporter maps `UNKNOWN_OUTCOME` to `reported: unknown`.

## Alternatives considered

OpenTelemetry spans as the record (one tree per trace, known durations); a full robotics ontology; schemaless JSON.
See ADR-0028.

## How it will be tested

Property tests: ingestion is idempotent and order-independent (any permutation and duplication of a batch gives the
same derived tables); conflicts are kept; rebuild equals incremental. Fixtures: the four Gazebo runs and the Málaga
recording imported end to end. Schema: round trip Rust ↔ JSON Schema ↔ Python. Migrations: every migration applied to
the previous release's fixture store.

## Open questions

1. Is a 64 KiB event limit too small for partners who put structured results (e.g. grasp scores) in events?
2. Do partners have stable task ids today, or must the importer synthesize them from action sequences?
3. Is per-tenant `source.id` uniqueness achievable across vendors' robots on one site?
