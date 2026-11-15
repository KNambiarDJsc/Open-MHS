# F. API specifications (draft for review)

## 1. Principles

- Instrumentation never breaks the robot: SDK calls do not raise into application code by default, do not block on
  I/O, and count what they drop.
- Every write is idempotent by event id. Every read says what it is based on (data cut-off, rules version).
- One schema (RFC 0005) for SDKs, importers, files and HTTP.

## 2. Python SDK (`openmhs.ops`)

```python
from openmhs import ops

rec = ops.Recorder(
    robot="robot-12", site="warehouse-east", tenant="acme",
    spool="/var/lib/openmhs/spool",          # default from $OPENMHS_SPOOL
    source="pick-app",                        # source.id; boot_id generated per process
    manifest="/etc/openmhs/manifest.json",    # optional: emits deployment.applied at start
    clock_domain="robot-12/chrony", uncertainty_ms=5.0,
)

with rec.task("pick-918", task_type="pick") as task:      # task.started / task.ended
    with task.action("grasp-42", component="manipulation", goal_id=goal_uuid) as act:
        ...
        act.fail(code="grasp.empty")                      # action.ended outcome.reported = failed
    task.fault("perception.depth_dropout", severity="warning")
    with task.intervention(category="takeover", operator="remote", operator_id="op-3"):
        ...
    task.artifact(uri="s3://.../run_0931.mcap", media_type="application/x-mcap",
                  range=(t0, t1), streams=["/scan"])
    task.succeed()   # or fail(...), or leave: an exception in the block ends the task as failed
                     # with the exception type as code (no message text: it may hold secrets)
```

| Concern | Behaviour |
|---|---|
| Sync / async | Context managers work in sync code; `async with` variants (`rec.atask`) for asyncio. Events are enqueued, never written on the caller's thread |
| Thread and process safety | One writer thread per `Recorder`; enqueue is a bounded `queue.SimpleQueue`-style buffer. Multiple processes write separate spool files (`<source>-<boot_id>-<n>.jsonl`), no locks across processes |
| Context propagation | `contextvars`: the current task/action is inherited by child calls and asyncio tasks; `rec.current_task()` |
| Batching | Flush every 200 ms or 256 events or on `task.ended`, whichever first; `fsync` on flush of critical events (task end, intervention, deployment, fault ≥ error) |
| Offline | Writes are local files; nothing needs a network |
| Failure | Queue full → drop `info` events first, then refuse non-critical, never block; every drop counted in `rec.stats()` and in the next file's header record. `strict=True` raises instead (tests) |
| OpenTelemetry | If `opentelemetry.trace` is importable and a span is current, its ids fill `trace` |
| ROS 2 extension | `openmhs.ops.ros2.ActionObserver(node, "/navigate_to_pose")` subscribes to the status topic and emits `action.*` keyed by goal UUID, without wrapping the user's action client |

The existing `openmhs.Client` (gateway JSON-RPC) is untouched.

## 3. Rust SDK (`openmhs-events`, later `openmhs-ops-sdk`)

```rust
let rec = Recorder::builder()
    .tenant("acme").site("warehouse-east").robot("robot-12")
    .source("pick-app").spool(Path::new("/var/lib/openmhs/spool"))
    .clock_domain("robot-12/chrony", Some(5.0))
    .build()?;                                  // spawns one writer thread

let task = rec.task("pick-918").task_type("pick").start();   // returns a guard
let act = task.action("grasp-42").component("manipulation").goal_id(goal).start();
act.end(Reported::Failed, Some("grasp.empty"));
task.end(Reported::Failed);                     // Drop without end() records `unknown`, reason `dropped`
```

`Recorder` is `Send + Sync`; guards are `Send`. Task-local propagation uses `tokio::task_local!` when the `tokio`
feature is on. The core types (`Event`, `Manifest`, validation, canonical JSON, digests) live in `openmhs-events` with
no async runtime dependency, so a robot's real-time process can construct events and hand them to a non-real-time
writer.

## 4. Ops HTTP API (ADR-0031)

Auth: `Authorization: Bearer <token>`. Token scopes: `tenant`, optional `sites[]`, `robots[]`, and roles
`ingest` (write events/manifests for its scope only), `read`, `annotate`, `admin`. A robot token can only write
events whose `scope.robot` is its own.

### 4.1 Ingest

`POST /v1/events:batch` (`Content-Type: application/x-ndjson` or JSON array), at most 1,000 events or 4 MiB.

```json
{ "results": [
  { "event_id": "0193a6f2-…", "status": "accepted" },
  { "event_id": "0193a6f3-…", "status": "duplicate" },
  { "event_id": "0193a6f4-…", "status": "conflict", "existing_hash": "sha256:…" },
  { "event_id": "0193a6f5-…", "status": "rejected", "error": { "code": "event.schema", "message": "time.clock_domain is required" } }
],
  "ingested_at": "2026-11-15T10:02:11.004Z", "rules_version": "corr-1" }
```

HTTP 200 whenever the batch was parsed, even if some events were rejected; 400 only for an unparseable batch; 413
over limits; 401/403 auth; 429 with `Retry-After` under load. A client marks an event sent only on `accepted`,
`duplicate` or `conflict`.

`POST /v1/manifests` (body: manifest) → `{ "digest": "sha256:…", "status": "accepted" | "duplicate" }`.
`POST /v1/artifacts` (body: artifact reference) → same shape.

### 4.2 Incidents

`GET /v1/incidents?robot=&site=&task_type=&status=&category=&manifest=&from=&to=&limit=&cursor=`

```json
{ "items": [ { "incident_id": "INC-7f3a…", "robot": "robot-12", "site": "warehouse-east",
    "task_id": "pick-918", "task_type": "pick", "opened": "…", "status": "open", "owner": null,
    "trigger": "task.failed", "reported": "aborted", "verified": null, "fingerprint": "pick/manipulation.grasp/aborted",
    "manifest": "sha256:9f2c…", "members": 7, "recurrences": 3 } ],
  "next_cursor": null, "as_of": "2026-11-15T10:02:11Z", "rules_version": "corr-1" }
```

`GET /v1/incidents/{id}/timeline` → ordered members, each with `basis` (`id_match`, `window`, `operator`, `hypothesis`),
`window_ms`, `concurrent_with[]`, plus `context` (manifest, `changed_since_last_success` diff, artifacts,
interventions).

`PATCH /v1/incidents/{id}` (`annotate` role): `owner`, `status`. `POST /v1/incidents/{id}/annotations`: text with
`provenance: operator` (a `generated` annotation must name the model and the event ids it used).

`GET /v1/incidents/{id}/similar` → matches with the fields that matched.

`POST /v1/incidents/{id}/handoff` `{ "format": "zip" | "lerobot-selection" }` → a package (Layer 9).

### 4.3 Release intelligence

`GET /v1/releases/compare?task_type=pick&a=<manifest|label>&b=<manifest|label>&from=&to=&strata=site&outcome=reported`

```json
{ "as_of": "…", "outcome_kind": "reported", "strata": ["site"],
  "arms": { "a": { "attempts": 1204, "failed": 31, "unknown": 4, "excluded": { "no_manifest": 12 },
                   "rate": 0.0258, "ci95": [0.0183, 0.0364], "robot_hours": 412.5,
                   "interventions_per_hour": 0.21, "ci95_interventions": [0.17, 0.26] },
            "b": { "attempts": 388, "failed": 19, "unknown": 9, "rate": 0.0501, "ci95": [0.0323, 0.077],
                   "robot_hours": 138.6, "interventions_per_hour": 0.44, "ci95_interventions": [0.34, 0.57] } },
  "difference": { "estimate": 0.0243, "ci95": [0.0036, 0.0522], "method": "newcombe",
                  "unknown_bounds": [0.0199, 0.0464], "stratified": false },
  "verdict": "margin_exceeded" | "no_change" | "improved" | "insufficient_evidence",
  "warnings": ["arm b ran only at site warehouse-east"],
  "representative_incidents": { "a": ["INC-…"], "b": ["INC-…"] } }
```

### 4.4 Query patterns and their indexes

| Question | Index (RFC 0005 §6) |
|---|---|
| Everything on robot R between t0 and t1 | `events_robot_time` |
| All events of task T / goal G | `events_task`, `events_goal` |
| All attempts of task type K under manifest M | `task_attempts` (rebuildable; add an index on `(tenant, task_type, manifest, started_ns)`) |
| Incidents with fingerprint F since date D | `incidents(fingerprint)` index |
| Events of type X over the fleet in a period | `events_type_time` |

## 5. CLI (Stage 1, works on the file)

```
openmhs ops ingest events.jsonl [--store ops.sqlite]
openmhs ops manifest push manifest.json
openmhs ops rebuild [--rules corr-1]
openmhs ops incidents [--robot R] [--since 7d]
openmhs ops show INC-…            # timeline (Layer 9)
openmhs ops compare --task pick --a 2026.11.1 --b 2026.11.2 --by site
openmhs ops handoff INC-… --out dir/
openmhs ops import-gateway ~/.openmhs/journal.sqlite
```

## 6. MCP (read-only)

Tools: `ops_list_incidents`, `ops_get_timeline`, `ops_compare_releases`. Same scopes as `read`. No write tools: an
agent may summarize, a person annotates. Outputs carry event ids so an agent's statements can be checked (ADR-0022's
rule: no device or agent text copied into structured fields).

## 7. Authorization boundaries

| Principal | May |
|---|---|
| Robot token | `ingest` for its own `robot` id only |
| CI token | `ingest` manifests and `deployment.*` for a tenant |
| Engineer | `read`, `annotate` within granted sites |
| Admin | tokens, retention, rules configuration |
| MCP client | whatever its token allows, read-only tools only |

Artifacts are never served by Open-MHS; links resolve with the user's own credentials to the storage system.
