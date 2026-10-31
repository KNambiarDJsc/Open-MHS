# OpenMHS system design (as implemented, 2026-10-30)

This document describes what is **built and running on `main`** (commit `0219a19` and later),
how a request flows through it, what each part guarantees, and what is not built. It is grounded
in the code, not in plans; planned work is listed separately at the end. OpenMHS is pre-alpha and
has **never been run against physical hardware**; every end-to-end result cited here is from
simulators.

## 1. What it is, in one paragraph

OpenMHS is a gateway between software that wants to change the physical world (AI agents,
scripts, dashboards) and the devices that do it. A caller asks for an operation on a device
capability. OpenMHS checks the request against the device's declared contract (types, units,
ranges, side effects) and against a deny-by-default policy, optionally holds it for a human,
serializes it with other work on the same device and shared resources, sends it through an
adapter, follows the device's acknowledgement, verifies the effect by reading the device back,
and records every step in a SQLite journal with a hash-chained audit trail. When it cannot tell
whether something happened, it says so (`UNKNOWN_OUTCOME`), holds related work, and never
resends on its own.

## 2. The parts

```text
 Agents & clients                        Gateway process (openmhs serve)                    Devices
 ───────────────                         ─────────────────────────────────                    ───────
 MCP agent ──stdio── openmhs mcp ──┐
 Python SDK (openmhs) ─────────────┤     ┌──────────── openmhs-gateway ───────────────┐
 CLI (openmhs …) ──────────────────┼─RPC─▶ authenticate token → session(principal,role,│
 any HTTP client ──────────────────┘     │ scopes) → dispatch 53 methods               │
                                         └──────────────┬─────────────────────────────┘
                                                        ▼
                                         ┌──────────── openmhs-engine ────────────────┐
                                         │ validate (types/units/ranges/preconditions)│
                                         │ authorize (openmhs-policy: scope ∩ grant)  │
                                         │ approvals · leases · plans · procedures    │
                                         │ per-device + shared-resource locks         │
                                         │ dispatch → follow → verify → reconcile     │
                                         │ availability · state fabric · insights     │
                                         │ journal (SQLite, WAL) + hash-chained audit │
                                         └──────────────┬─────────────────────────────┘
                                                        ▼
                          in-process adapters (Rust)            out-of-process adapters (any language)
                          http · modbus · mqtt · scpi ·         openmhs-host supervises a child process
                          profile-driven · built-in sims        over a JSON protocol + permission manifest
                                │                               (Python SDK: WoT/LabThings, ROS 2,
                                ▼                                OPC UA, OctoPrint, PSU sim)
                          the device / controller                        │
                                                                         ▼
                                                                  the device / controller
```

| Crate / package | Lines* | Responsibility |
|---|---:|---|
| `openmhs-types` | 4,472 | The data model: descriptors, commands, receipts, the lifecycle state enum and its transition table, units, values, topology, plans, procedures, evidence bundles and their offline verifier, JSON Schemas, canonical JSON |
| `openmhs-policy` | 1,485 | Authentication material, credential scopes, grants with glob matching, constraint checks; decides allow / deny / require-approval |
| `openmhs-engine` | 6,979 | The execution kernel: lifecycle, locks, dispatch, verification, reconciliation, recovery at start, approvals, leases, plans, availability, events, insights, twin preflight, and the SQLite journal |
| `openmhs-adapter` | 218 | The device-boundary trait (`HardwareAdapter`: describe, connect, health, read, execute, shutdown) and the progress channel (`accepted`, with an optional device reference) |
| `openmhs-host` | 2,672 | Runs adapters as separate processes: launch, supervise, restart, a line-delimited JSON protocol, permission manifests (network endpoints, files, credentials by name), adapter signing trust |
| `openmhs-gateway` | 2,229 | HTTP JSON-RPC server: token authentication, role (agent/operator), 53 RPC methods mapped onto the engine |
| `openmhs-api` | 1,991 | Shared RPC types and the Rust client |
| `openmhs-mcp` | 2,356 | The Model Context Protocol server: 23 tools for agents, with guidance text and untrusted-text marking |
| `openmhs-cli` | 5,301 | The `openmhs` binary: 28 commands (serve, demo, bench, discover, exec, approvals, grants, tokens, audit export/verify-file, plans, procedures, mcp, conformance, …) |
| `openmhs-sim` | 1,453 | Built-in simulated devices (power supply, axis, …) with fault injection |
| `openmhs-conformance` | 2,850 | The conformance suite adapters are measured against |
| `adapters/openmhs-{http,modbus,mqtt,scpi,profile}` | 5,959 | Rust protocol adapters, mostly driven by declarative device profiles |
| `adapters/python/*` | 4,224 | Python adapter SDK users: `wot` (Thing Description importer + LabThings/HTTP adapter), `ros2` (observation-only), `opcua`, `octoprint`, `psu-sim` |
| `python/openmhs` | 2,961 | Python client SDK (`Client`, `execute()` with recovery by idempotency key) and the agent harness (`openmhs.agent`: MCP client, OpenAI-compatible and Anthropic providers) |

\*Non-test source lines.

## 3. Contracts: what a device is allowed to be

A **descriptor** (schema `spec/schemas/descriptor.schema.json`) is the contract for one device.
Each **capability** has a kind (property or action), operations (`read`, `write`, `invoke`,
`subscribe`), a typed value or parameters (with unit and range for quantities), a declared
`side_effect` (`none`, `reversible`, `irreversible`, `hazardous`), `idempotent`, timing bounds,
preconditions, and a **verification** rule (`readback` from a named capability with a tolerance,
or `none` with a stated reason). Descriptors are validated strictly at registration
(unknown fields refused, `x-` extensions only, single-capability verification, quantities as
`{value, unit}`), and must not change while the gateway runs. Registration records the
descriptor's SHA-256 in the audit trail.

**Topology** (`spec/topology.md`) declares relationships and shared resources between devices
(for example two devices on one bus or one stage). The engine serializes on those resources.

**Importing a W3C Thing Description** (`adapters/python/wot/`): readable properties become read
capabilities automatically; writable properties stay read-only; **actions stay hidden until a
person reviews them** and supplies side effect, idempotency, limits, timing and verification in an
override file. The importer records the source description's hash in the descriptor. Measured on
the OpenFlexure simulator: 83 capabilities generated, 0 of 50 actions exposable without review.

## 4. Who may do what

- **Credentials** are bearer tokens with an id, a principal (`agent:…`, `operator:…`), a role, a
  set of **scopes** (what the credential may ever be used for) and an expiry. Tokens are stored
  hashed; rotation and revocation exist.
- **Grants** (policy) match principal, device and capability globs and operations to an effect:
  `allow`, `deny`, or `require_approval`. Deny by default: a request needs both a scope and a grant.
- **Approvals** hold a command in `AWAITING_APPROVAL` until an operator approves (it then runs),
  denies or lets it expire. Approvals can also be requested ahead of submission and are spent once.
- **Discovery is advisory**: `access.allowed_operations` (`get_allowed_operations` in MCP) answers
  "what may I do" for the caller, with policy and descriptor digests and a 10-second expiry. The
  engine re-checks every command at dispatch regardless.
- Agents never see the operator token; operator-only RPCs (grants, tokens, approvals, audit
  export, resolve) refuse an agent credential (`gateway.forbidden`).

## 5. The life of a command

Submitted as `{target: {device, capability}, operation, value | params, idempotency_key?, deadline_ms?}`.

```text
CREATED → VALIDATING ─┬─► DENIED                         (contract, policy, precondition or limit)
                      ├─► AWAITING_APPROVAL ─┬─► APPROVED (operator) …
                      │                      └─► DENIED / EXPIRED / CANCELLED
                      └─► APPROVED → DISPATCHED ─┬─► REJECTED            (device refused: nothing done)
                                                 ├─► FAILED / TIMED_OUT
                                                 ├─► UNKNOWN_OUTCOME ──► RECONCILED_APPLIED
                                                 │                       RECONCILED_NOT_APPLIED
                                                 └─► ACCEPTED → APPLIED ─┬─► VERIFIED
                                                                         ├─► VERIFICATION_FAILED
                                                                         └─► UNVERIFIABLE
```

19 states, one transition table in `crates/openmhs-types/src/lifecycle.rs`, tested exhaustively and
with generated histories. What each step does:

1. **Idempotency.** `(principal, device, idempotency_key)` is unique in the journal. The same key
   with the same request returns the stored receipt and runs nothing; the same key with a different
   request is refused. The SDK's `execute()` and `commands.get_by_key` recover a receipt whose HTTP
   response was lost, without resubmitting.
2. **Validation.** Types, units, ranges, rate and slew constraints, descriptor preconditions (read
   live), and the unresolved-outcome hold (below). Failures become `DENIED` with structured reasons
   and next steps; nothing reaches the device.
3. **Authorization** (scope ∩ grant) and, if required, approval.
4. **Serialization.** The command takes its device lock and the locks of every shared resource its
   topology declares, in a fixed order, and holds them from the live checks through verification.
   Declared **stops** bypass the queue on a separate lane.
5. **Dispatch.** The `DISPATCHED` transition is journaled before the adapter is called. The adapter
   reports `accepted` (optionally with the device's own reference, such as a LabThings invocation
   id, stored in the receipt and in the chained audit), then success, refusal or error.
6. **Classification of failures.** A refusal before acceptance is `REJECTED`. A timeout, a lost
   connection, a malformed reply, a crashed adapter or a restarted gateway during execution is
   `UNKNOWN_OUTCOME`: the effect may or may not have happened. An error after acceptance is never
   reported as "not sent".
7. **Verification.** `APPLIED` is the device's own claim and is not success. `VERIFIED` requires a
   readback that matches within tolerance **and was measured after dispatch** (ADR-0020;
   `--readback-skew-ms` allows for clock gaps). No usable fresh reading gives `UNVERIFIABLE`; a
   mismatch gives `VERIFICATION_FAILED`.
8. **Uncertainty is held, not guessed.** While a command is `UNKNOWN_OUTCOME`, new writes to the
   same capability, and to devices sharing its declared resources, are refused
   (`command.outcome_unresolved`) unless idempotent or a stop. `commands.reconcile` decides from a
   post-dispatch readback (`RECONCILED_APPLIED` / `RECONCILED_NOT_APPLIED`); `commands.resolve` lets
   an operator settle it with a recorded reason when no readback can.

## 6. Durability and recovery

The journal is one SQLite database (WAL mode) in the gateway home. Tables: `commands` (state,
request hash, serialized receipt), `approvals`, `leases`, `plans`, `plan_commands`, `audit`. Each
command transition updates the receipt and appends its audit record **in one transaction**.

On start, recovery walks unfinished commands. Anything that might have reached a device becomes
`UNKNOWN_OUTCOME` (never resent); `APPLIED` resumes verification once the device is registered;
pre-dispatch work is closed or left for a person. Tested by reopening the journal at eight persisted
states and recovering twice with zero device calls (`recovery_windows.rs`), and by killing the
gateway process mid-command.

A journal write that fails mid-command (an I/O error, a full disk) does not end the call early: the
device stays locked until the adapter call ends, and the command is settled as `UNKNOWN_OUTCOME`
(or `EXPIRED` / `FAILED adapter.not_sent` if never sent). One that cannot even be settled stays in
flight on record and is held like an unknown outcome until recovery.

The gateway does not start on a journal it cannot trust: one that fails SQLite's integrity check
(including the unique indexes behind idempotency and approvals) or whose audit chain does not
verify. The readback target of each command is bound to the chain by its `DISPATCHED` record.
With `serve --checkpoint-file`, the audit head is also written outside the journal after every
dispatch, before the device is called, so a journal rolled back to an older copy is refused at
start instead of believed.

The persistence drills (48 tests; report in [PERSISTENCE_ADVERSARY.md](PERSISTENCE_ADVERSARY.md))
fault every durable write of commands, plans and leases before and after its commit, crash
recovery itself, sweep the journal's size limit, abort a real process, tear the write-ahead log
mid-write, flip bits across the database file, and roll the journal back. They found and fixed
four defects, among them a journal failure that let the next write on the same capability through
and a by-key receipt lookup that returned damaged receipts unchecked.

**Storage limits** (ADR-0021): `--journal-max-mib` and a reserve. Inside the reserve, new commands
and audited reads are refused with `storage.exhausted` *before* anything is sent, so no action runs
without a record (measured: 448 commands, 448 device jobs, then refusal).

## 7. Audit and evidence

- Every security-relevant event (registrations, commands and transitions, reads, grants, tokens,
  approvals, plans) is an audit record `{seq, at, kind, subject, data, prev_hash, hash}`; each hash
  covers the previous one.
- Command records carry a **digest of the full receipt**; a receipt edited after the fact is refused
  on the next read or transition (`journal.receipt_integrity`).
- `openmhs audit export` writes a portable evidence bundle (whole chain + all receipts + head), and
  `openmhs audit verify-file` checks it **offline**, with no gateway or database, optionally against a
  checkpoint kept elsewhere. It reports `valid` and `complete` separately.
- Limits, stated plainly: the chain detects edits and deletions in the middle; it does **not** stop
  someone who can rewrite the file from recomputing every hash, and it does not reveal removal of the
  newest records unless an independently retained checkpoint is compared. Checkpoints are unsigned.
  It records what devices reported, not physical truth.

## 8. Doing more than one thing

- **Leases**: exclusive use of devices and resources for a time, by a holder.
- **Plans** (`spec/plans.md`): up to 100 steps across devices under one lease, ordered by
  dependencies, each an ordinary command with every check above; preconditions and conditional
  `when` steps (ADR-0024); opt-in parallel steps where devices do not overlap (ADR-0025). On failure:
  halt, compensate (undo completed steps, newest first) or ask. **An uncertain step always stops the
  plan for a human decision** (`resume`, `retry`, `compensate`, `abort`), whatever the plan says.
- **Twin preflight** dry-runs a plan against twins of its devices (no physics).
- **Procedures** (`spec/procedures.md`): reviewed, versioned plans with typed parameters that agents
  may run but not edit; agents may propose new ones for an operator to approve.

## 9. Seeing what is happening

- **Availability** (ADR-0019): devices are probed; one that does not answer at start registers
  `OFFLINE` instead of stopping the gateway; reads report quality and age.
- **State fabric** (ADR-0023): `events.poll` streams observations, availability changes and command
  transitions from a bounded buffer, reporting drops; `watches` sample subscribable capabilities.
- **Insights** (ADR-0022): a read-only summary of a principal's past commands on a device (confirmed
  values, refusals with the limits behind them, tracking error, unresolved outcomes). It never
  changes a limit or a grant.

## 10. The agent interface

`openmhs mcp` serves 23 tools: `list_devices`, `describe_device`, `find_capabilities`,
`get_allowed_operations`, `read_state`, `watch_state`, `cancel_watch`, `poll_events`,
`check_command`, `submit_command`, `get_command`, `cancel_command`, `reconcile_command`,
`list_pending_approvals`, `get_insights`, `check_plan`, `submit_plan`, `get_plan`, `cancel_plan`,
`list_procedures`, `check_procedure`, `run_procedure`, `propose_procedure`. Text that comes from
devices is marked as untrusted. Tool results carry `confirmed`, structured `reasons` and
`next_steps` (for example "never resubmit; call reconcile_command"). `openmhs mcp --sim` brings its
own throwaway simulated lab. `openmhs.agent` is a reference harness that runs a model over these
tools with a confirmation hook.

**OpenFlexure agent kit** (`examples/openflexure-agent/`): one command imports the simulator's
stage with a reviewed envelope, starts a gateway, grants one agent bounded moves and writes an MCP
configuration with two servers: OpenMHS and a small imaging server (`capture_image`,
`survey_region`, `focus_sweep`, `session_report`) whose moves go through OpenMHS with the agent's
token.

## 11. How a real session goes (recorded, simulator)

A technician asked an agent (GPT-6 Luna, both servers attached): *"find where the sample is:
survey a 5×5 area … refocus on the best tile and give me a session report"*.

1. The agent calls `survey_region(5, 5, 500)`. For each of 25 tiles the imaging server moves the
   stage axis by axis through OpenMHS (`commands.submit` with the agent token): validated against
   the ±1200 envelope, authorized by the agent's grants, serialized on the stage, dispatched to the
   generated LabThings adapter, which POSTs the action, records the invocation id on acceptance,
   follows the invocation to completion, and reports; the engine then reads `position.x` back and
   marks the move `VERIFIED`. Only then is an image captured and measured.
2. `focus_sweep` does the same for five z positions and moves to the sharpest.
3. `session_report` lists every move with its command id and state, every image with position,
   measurements and sha256, exports the audit evidence with the operator credential, and verifies
   it offline.
4. Result: 34 moves all `VERIFIED`, 30 images, audit verified (exit 0), no unresolved outcome; the
   agent's answer matched the tool output number for number.

When the acknowledgement of one move was dropped in a scripted run, the move came back
`UNKNOWN_OUTCOME`, the workflow suspended, reconciled from readback (`RECONCILED_APPLIED`) and
continued; the simulator's own records show the move ran exactly once.

## 12. Guarantees and their scope

| Holds (tested, single gateway) | Does not hold |
|---|---|
| No dispatch without a scope, a grant and (if required) an approval | Protection against someone with write access to the journal or the host |
| No value outside a declared range or type reaches the adapter | Correctness of limits a reviewer set wrongly or left open (`unbounded_ok`) |
| No automatic resend after an ambiguous dispatch; unresolved outcomes hold related writes | Exactly-once execution on devices that cannot deduplicate |
| No `VERIFIED` without a post-dispatch matching readback | Physical truth: a device that reports wrongly is believed |
| One writer per device and declared resource **within one gateway process** | Fencing across processes, gateways, or a direct controller (no ownership epochs yet) |
| Admitted work survives a gateway restart, a journal write failure, a torn write or a damaged byte; nothing is resent; a damaged journal is refused at start | A rolled-back journal without `--checkpoint-file`; storage that does not honour syncs (power loss on real hardware not drilled); disk loss |
| Receipts bound to the audit chain; offline verification of exports | Tamper-proofing; signed or externally anchored checkpoints |
| Readings without a device timestamp can use the HTTP `Date` header | Staleness shorter than about a second with that mitigation |

## 13. Evidence that it works (all simulator)

- **Tests:** 507 Rust tests (83 binaries, including 48 persistence drills), clippy `-D warnings` clean; 59 Python SDK tests; 25 Thing
  Description importer tests; CI on Linux, Windows and macOS plus ROS 2, OPC UA and OctoPrint
  simulator jobs.
- **Research** (`research/`): failure scenarios against LabThings asynchronous actions with ground
  truth from the server; a 225-run fault-injection benchmark (OpenMHS removes duplicate and
  out-of-limit dispatch; naive clients complete less; a careful hand-written client matches it on
  duplicates); audit tamper experiments; a two-device plan with a fault; a second, independently
  described device; model runs on the OpenFlexure simulator; the imaging workflow and the
  technician session above.

## 14. Not built

In order of the migration plan (`docs/architecture/DISTRIBUTED_MIGRATION_PLAN.md`):

1. **Ownership epochs** and stale-owner refusal, enforced at the device where the controller can.
2. What the persistence drills cannot reach in software: hard power loss on real storage (a VM
   or power-cut rig), I/O errors from a faulty disk (a Linux `dm-flakey` job in CI), and the
   start-up cost of the integrity checks on a large journal.
3. Signed checkpoints, automated external anchoring, per-person operator identity, software hashes
   in the audit.
4. A distributed mode: central admission ledger and outbox (PostgreSQL), edge inbox and status
   sync, a transport chosen after measurement (gRPC or NATS JetStream), workload identity and mTLS,
   OpenTelemetry. None of it is required for, or may replace, the single-gateway mode.
5. Product gaps for real users: a read-only first mode for a real microscope, image capture as a
   first-class audited capability, a release containing this work (with a 64-bit ARM Linux build
   for Raspberry Pi), the Python SDK on PyPI, and supervised physical validation for each device
   class.

## 15. Where to read more

`spec/` (lifecycle, authorization, plans, procedures, topology, adapter protocol, descriptor,
errors), `docs/adr/` (25 decisions), `docs/architecture/` (kernel, state machine, invariants,
audit evidence, validation, migration), `research/README.md` (every experiment), and
`examples/openflexure-agent/README.md` (try it).
