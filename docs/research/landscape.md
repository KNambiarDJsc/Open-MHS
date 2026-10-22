# Landscape: AI agents ↔ physical hardware (as of 2026-10-23)

This document records what already exists, what is unsolved, and where OpenMHS
adds independent value. Claims are sourced; anything not independently verified
is marked as such. Repository metrics are point-in-time observations.

## 1. Anthropic's Model Hardware Standard (MHS)

- Announced 2026-08-27 as a **limited research preview**, access by application.
  Started as a project between Anthropic and HHMI Janelia Research Campus.
  [modelhardwarestandard.com](https://www.modelhardwarestandard.com/),
  [CNBC](https://www.cnbc.com/2026/08/27/anthropic-pushes-into-physical-world-with-new-standard-to-help-ai-agents-operate-machines.html),
  [Fortune](https://fortune.com/2026/08/27/anthropic-makes-first-move-into-physical-ai-with-universal-standard-for-scientists-manufacturing/)
- Described in press coverage as: a per-device driver exposing basic read/modify
  operations in a common, discoverable format; drivers carry reference information
  such as safety limits; model-agnostic; works with any device that has a
  programmable interface; positioned as extending the MCP approach to physical
  machines. [The Decoder](https://the-decoder.com/anthropic-wants-to-do-for-physical-hardware-what-its-model-context-protocol-did-for-software/)
- **The specification is not public.** The official site publishes no spec, wire
  format, or license; it states an intent to open-source after the preview, with no
  date. `github.com/modelhardwarestandard` exists with no public repositories (per
  [awesome-mhs](https://github.com/carllippert/awesome-mhs)).
- Reported partner results (integration time "weeks → hours", CMU ~8 h, QuEra
  695/700) come from Anthropic/partner presentations and are **not independently
  verified**. Reported limitation: weak physical cause-and-effect reasoning
  (Genentech foaming-assay anecdote). [The Decoder](https://the-decoder.com/anthropic-wants-to-do-for-physical-hardware-what-its-model-context-protocol-did-for-software/)

**Implication:** OpenMHS cannot claim MHS compatibility. It must be useful on its
own and be structured so an MHS-compatible façade can be added once (if) the spec
is published (see ADR-0007).

## 2. The "open MHS" ecosystem that appeared in the two months since

[awesome-mhs](https://github.com/carllippert/awesome-mhs) lists ~20 community
projects. Representative ones, inspected directly:

| Project | Stack | What it does | Maturity signals |
|---|---|---|---|
| [Abenor-Labs/Open-MHS](https://github.com/Abenor-Labs/Open-MHS) | Python, FastAPI, JSON-RPC, JSON Schema 2020-12 | Capability tag with limits; server + driver double-check; read/write/e-stop; MCP (7 tools), HTTP, CLI; hash-chained JSONL audit; G-code serial driver; MuJoCo twin | Self-reported 372 tests; alpha v0.1; single maintainer; in-memory registry; tags unsigned; one shared token; serial driver never run on real hardware; Apache-2.0; 6★ |
| [vraj130/open-mhs](https://github.com/vraj130/open-mhs) | Python, Pydantic, Typer | Manifest → MCP tools; read/write/invoke/estop; firmware + host interlock + confirm flag | 1507 tests self-reported; Pico hardware path never run on a board; MIT; 0★ |
| [huzjie/hardmesh](https://github.com/huzjie/hardmesh) | Python stdlib, FastAPI optional, React console, K8s/Helm | Broad platform: agent loop, workflows, calibration, many protocol drivers, default-deny, N-of-M approval | 2 commits; mock driver default; connects to no real devices; Apache-2.0; 0★ |
| Others on the list | mostly Python | Fastly `edge-mhs` (MCP edge gateway with limits/quotas/audit — could not locate the repo to verify), A3S-Lab/MHS (deterministic virtual lab), TARE (readback-verified adapters, MIT), mhs-interop-bridge (SiLA 2 drivers, 23 fault scenarios), Qwen MHS adapters | listed, not individually verified |

Observed pattern across these projects:

1. Nearly all are **Python, simulated-only**, and have **never been run against real
   hardware** by their own admission.
2. The common feature set is the same: a device manifest with limits → MCP tools →
   read/write/e-stop → audit log.
3. Command semantics stop at "allowed / blocked / returned". None that we inspected
   documents a durable command journal, crash recovery, idempotency keys, or a
   formal `UNKNOWN_OUTCOME` + readback reconciliation model (TARE advertises
   readback verification; not inspected in depth).
4. No shared, implementation-neutral **conformance suite** exists; each project
   self-reports its own test counts.
5. The name is saturated: `open-mhs`/`OpenMHS` is used by at least six unrelated
   repositories (see [naming.md](naming.md)).

## 3. Established standards OpenMHS must build on, not replace

| Standard | What it already solves | What it leaves to us |
|---|---|---|
| **MCP** | Agent ↔ tool calling, tool schemas, transports | Physical semantics: units, limits, freshness, uncertain outcomes, approvals |
| **ROS 2 / ros2_control** | Robot middleware; hardware components (sensor/actuator/system), controller lifecycle, real-time loops | Agent-level authorization, supervisory command tracking, evidence |
| **OPC UA** | Rich information models, companion specs (machinery, robotics), security, value quality codes | Agent-facing capability projection, policy, audit; we borrow its GOOD/UNCERTAIN/BAD quality concept |
| **SiLA 2** | Lab device features and commands, standardized | Cross-domain (non-lab) model, agent policy |
| **Modbus / SCPI / MQTT / serial** | Raw transport to vast installed base | All semantics: register meaning, units, limits come from device profiles |
| **Home Assistant** | Huge consumer-device integration catalog, entity model | Industrial/lab semantics, deterministic policy & audit |
| **OpenTelemetry** | Traces/metrics/logs export | Hardware-specific audit semantics |

## 4. Where OpenMHS adds independent value

Given the crowded "manifest + MCP + limits" space, OpenMHS should **not** compete on
that feature list. It should compete on things the existing projects do not do:

1. **Execution correctness under real-world failure.** A durable command journal,
   intent records, idempotency, crash recovery, and an explicit
   `ACCEPTED ≠ APPLIED ≠ VERIFIED` lifecycle with `UNKNOWN_OUTCOME` reconciliation.
   This is the hard part of talking to physical machines over unreliable links.
2. **A single trusted gateway binary** (Rust) that every interface — CLI, MCP,
   Python, future HTTP — must go through. Structural guarantee, not convention.
3. **Protocol adapters tested against protocol-level simulators** (real Modbus TCP,
   MQTT, SCPI over sockets) rather than in-memory fakes, so the code path that
   talks to real devices is the code path under test.
4. **An implementation-neutral conformance suite** with defined levels (L0–L3).
   In a field where every project self-reports, a neutral test harness others can
   run is the most plausible route to becoming shared infrastructure.
5. **Hardware-agnostic core enforced by test** — three dissimilar device classes
   through one unmodified core from day one.

## 5. Reusable libraries (verified on crates.io 2026-10-23)

| Need | Crate | Latest stable |
|---|---|---|
| MCP | `rmcp` (official Rust SDK) | 3.6.0 (updated 2026-10-20) |
| JSON Schema 2020-12 | `jsonschema` | 0.60.0 |
| SQLite | `rusqlite` (bundled) | 0.40.2 |
| Authorization (candidate) | `cedar-policy` | 4.14.0 |
| Local HTTP API | `axum` | 0.8.9 |
| Modbus (M6) | `tokio-modbus` | 0.17.0 |
| MQTT (M6) | `rumqttc` | 0.25.1 |

## 6. Open questions carried forward

- If Anthropic publishes MHS, does it define a wire format, a descriptor format,
  or both? ADR-0007 keeps the core independent so either can be mapped.
- Fastly `edge-mhs` is listed as an MHS-over-MCP gateway; the repository could not
  be located to verify scope. Re-check before positioning the gateway publicly.
