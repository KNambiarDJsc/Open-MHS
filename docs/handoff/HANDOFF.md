# OpenMHS handoff (Claude Code to Codex)

Written 2026-10-30 because the previous agent's usage budget ran out. This is the single entry
point: what OpenMHS is, what state the repository and machine are in, what was just done, what to
do next, and the rules that apply. Everything claimed here was observed in this repository unless
marked *unverified*.

## 0. Where everything is (this machine)

| What | Location | In git? |
|---|---|---|
| The project (repo, branch `research/labthings-wot`) | `C:\Users\HP\Open-MHS` | yes (`origin` = https://github.com/KNambiarDJsc/Open-MHS) |
| Handoff docs (this file, the brief, diagrams) | `C:\Users\HP\Open-MHS\docs\handoff` | yes |
| Research study (docs, harness, results) | `C:\Users\HP\Open-MHS\research` | yes |
| Research environment: venv (labthings-fastapi 0.3.0, OpenFlexure server), `ofm-sim`, `labbench`, gateway homes `work\`, logs, copied gateway binary `bin\openmhs-new.exe` | `D:\openmhs-research` | **no** |
| Private Ollama model store (qwen2.5:3b), server log | `D:\ollama-models` | **no** |
| Old release-build cargo target (the maintainer may delete) | `D:\openmhs-release-target` | **no** |
| Claude Code session transcript (full history of the study) | `C:\Users\HP\.claude\projects\C--Users-HP\3aa0011c-3129-4e49-884b-2efaf7c56f40.jsonl` | no |
| Claude's project memory | `C:\Users\HP\.claude\projects\C--Users-HP\memory\` | no |
| Stale code graph (git-excluded; report copied to `docs/handoff/graphify`) | `C:\Users\HP\Open-MHS\graphify-out` | no |

The D: items are not needed to read or build the repository, only to re-run the study's
experiments (section 8). They hold several GB; do not copy them into the repository.

## 1. Read in this order

1. This file.
2. [CODEX_PROMPT_DISTRIBUTED_ARCHITECTURE.md](CODEX_PROMPT_DISTRIBUTED_ARCHITECTURE.md): the
   maintainer's architecture analysis and the implementation prompt for the next phase. **This is the
   task.** Diagrams: [images/](images/).
3. [../../research/DECISION_MEMO.md](../../research/DECISION_MEMO.md) and
   [../../research/README.md](../../research/README.md): the evidence the next phase is based on.
4. Repository docs: `docs/concepts.md`, `docs/adr/` (ADR-0001 to 0025, notably 0018 orchestration,
   0019 availability, 0020 readback must follow dispatch, 0021 journal quota, 0023 state fabric),
   `spec/` (lifecycle.md, authorization.md, plans.md, adapter-protocol.md, descriptor.md),
   `docs/strategy/PLATFORM_ARCHITECTURE.md`, `docs/limitations.md`.
5. Code map: [graphify/GRAPH_REPORT.md](graphify/GRAPH_REPORT.md). **Stale** (built 2026-10-28 at
   commit `e14485cf`, before the research branch). Regenerate with the graphify skill
   (`graphify install --platform codex`, then `/graphify`). `graphify-out/graph.json` (6 MB) is
   git-excluded on purpose. `graphify path "A" "B"` and `graphify explain "X"` query it locally.

## 2. What OpenMHS is

Rust gateway between AI agents and hardware. Repo `KNambiarDJsc/Open-MHS`, local
`C:\Users\HP\Open-MHS`, Apache-2.0, **pre-alpha, never run against physical hardware**. Every
command is validated against a device's declared descriptor (units, ranges, side effects),
authorised deny-by-default (grants, scopes, optional human approval), dispatched through an adapter,
verified by readback where possible, and recorded in a SQLite journal with a hash-chained audit.
Lifecycle: CREATED, VALIDATING, APPROVED, DISPATCHED, ACCEPTED, APPLIED, VERIFIED; plus DENIED,
UNVERIFIABLE, VERIFICATION_FAILED, TIMED_OUT, UNKNOWN_OUTCOME, RECONCILED_APPLIED /
RECONCILED_NOT_APPLIED. An unresolved outcome holds the device until reconciled or resolved.

| Crate | Role |
|---|---|
| `openmhs-types` | descriptor, command, receipt, schema types; canonical JSON |
| `openmhs-policy` | grants, scopes, approvals decision |
| `openmhs-engine` | lifecycle, journal (SQLite), plans, leases, availability |
| `openmhs-adapter` / `openmhs-host` | adapter trait + out-of-process host, manifests, permissions |
| `openmhs-api` / `openmhs-gateway` | local HTTP API, tokens |
| `openmhs-mcp` | MCP server (22 tools) |
| `openmhs-cli` | `openmhs` binary |
| `openmhs-sim`, `openmhs-conformance` | virtual devices, conformance suite |
| `adapters/*` | http, modbus, mqtt, scpi, profile, python SDK; **`adapters/python/wot`**: Thing Description importer + LabThings/HTTP adapter (new) |
| `python/openmhs` | client SDK, agent harness (`openmhs.agent`) |

## 3. Repository state

- Remote: `origin` = https://github.com/KNambiarDJsc/Open-MHS. `main` = `60059df`, which has
  **v0.1.0 published** as a GitHub pre-release (PyPI not published).
- PR #7 (`dx/mcp-sim`): MCP `--sim`, insights, state fabric, harness, procedures, conditional and
  parallel plan steps, bench, discovery, RFC 0001. Open, not merged.
- **`research/labthings-wot`** is the working branch: 25 commits ahead of `main` as of `5539d19` (it contains all of
  `dx/mcp-sim`). Head at the time of writing: see `git log -1`. Its own commits are the LabThings
  study (importer, adapter, `external_ref`, provenance, research harness and results).
  Test status at head: **Rust 444 passed, 0 failed, 77 test binaries; clippy `-D warnings` clean;
  `cargo fmt --check` clean; Python 55 passed; importer 25 passed** (offline).
- Core changes on this branch, all tested: `ProgressEvent::Accepted { external_ref }`
  (adapter SDK, host protocol, receipt, schema), the `ACCEPTED` audit record carries
  `device reference <ref>`, `device.registered` records `descriptor_sha256`
  (`crates/openmhs-engine/tests/provenance.rs`).
- Do not rewrite history; do not force-push.

## 4. What the study found (details in `research/`)

All simulator-only; no hardware claim exists or should be made.

- **Benchmark** (225 runs, `research/RELIABILITY_BENCHMARK.md`): naive HTTP retry duplicated work in
  15 of 45 runs; OpenMHS arms with a stable idempotency key had none; out-of-limit target executed by
  plain HTTP, refused by OpenMHS 15 of 15; naive OpenMHS clients completed 5 and 10 of 45 vs 35 for
  HTTP (they stop where unsure), a receipt-heeding reconciling client 20 of 45 with no duplicates; a
  careful 49-line direct client matched OpenMHS on duplicates. Reliability is a trade of completion
  for certainty.
- **Audit** (`research/AUDIT_AND_PROVENANCE.md`): edits and middle deletions detected; a rewrite that
  recomputes hashes is not; tail truncation not detected without an external anchor; the receipt row
  is not covered by the chain (detected only by the export's receipt-vs-chain cross-check).
- **LabThings** (0.3.0): invocations live in memory and vanish on restart; expiry is lazy (only when
  another action is invoked); unknown and expired are both 404; property reads carry no measurement
  time (the HTTP `Date` header mitigates with a ~1 s blind spot); cancel is a request, not a stop.
- **Importer**: OpenFlexure sim: 83 capabilities generated, 0 of 50 actions declare `safe` or
  `idempotent`, 0 units; fails closed.
- **Agents**: a 3B model on CPU could not use either tool layer; no accuracy claim is possible. A 9B
  run was not feasible on this machine.
- **Three unresolved agent-tool defects (P0 in the brief):** `sim.axis-01` example id in a tool
  description (`crates/openmhs-mcp/src/plans.rs` near line 47) copied by the model 4/4 times;
  `get_insights` (`crates/openmhs-mcp/src/lib.rs`) returns an empty success for an unknown device;
  no tool answers "what may I do" (`find_capabilities` lists contract capabilities, not grants;
  `check_command` answers per command).

## 5. Changes carried by the maintainer's latest prompt

Beyond the study, the maintainer has now set the direction: a distributed architecture with a
central control plane (PostgreSQL ledger + outbox, workflow coordinator, contracts and policy)
and per-site edge gateways (local WAL journal, policy snapshot, fencing, device adapters), durable
messaging with mTLS, single-writer resource ownership with fencing epochs, a formal command
envelope and state table, strengthened audit (receipt digests, signed checkpoints, external anchors,
independent verifier), OpenTelemetry, chaos/property testing, and ADRs under
`docs/architecture/`. Four deployment levels (embedded, one lab, multi-site, industrial) with the
rule that **the higher levels must never be required for Level 1**. Release gates are listed in the
prompt file. Priority order is also set there: correctness fixes first, then the command protocol
and ledger, then ownership and the edge gateway; **do not install NATS, Temporal, PostgreSQL and
Kubernetes at once**, and do not rewrite working components into services.

## 6. Recommended first moves

1. Phase 0: write the audit from the code (module graph, current state machine in
   `crates/openmhs-types` and `crates/openmhs-engine`, journal schema in
   `crates/openmhs-engine/src/journal*`, locking, retry semantics). Classify each proposal
   necessary now / later / unnecessary.
2. Phase 1: fix the three P0 defects with tests (placeholders instead of concrete ids; typed
   `DEVICE_NOT_FOUND` for unknown devices across MCP tools; `get_allowed_operations` with policy
   revision and expiry; structured refusals). Re-run the research harness afterwards to confirm no
   regression (section 8).
3. Then the P1 audit items (receipt digests, checkpoints, anchor) before any distributed work.

## 6b. Existing behaviours to preserve

Unresolved-outcome hold, idempotency keys, ADR-0020 (readings must postdate dispatch; flag
`--readback-skew-ms`), ADR-0021 (journal limit and reserve), deny-by-default, `not_sent` must never
be reported after acceptance, adapter errors map as: timeout, disconnect, malformed reply become
UNKNOWN_OUTCOME; descriptors must not change between restarts; extension keys start `x-`.

## 7. Standing rules from the maintainer

- **Never push, merge, tag, publish, delete unrelated files, modify external repositories or contact
  researchers without explicit approval.** (On 2026-10-30 the maintainer approved pushing the
  `research/labthings-wot` branch and these handoff docs; that approval covers that push only. Merging
  PR #7, tagging, publishing and contacting Dr Richard Bowman / LabThings / OpenFlexure remain
  unapproved. `gh pr merge` is blocked by the environment's permission classifier; the maintainer
  merges.)
- No claim of hardware compatibility, physical validation, safety or certification. Keep simulator and
  physical claims separate. Do not claim OpenMHS improves agent accuracy: measured, it did not.
- No claim of an official collaboration, endorsement or involvement by Dr Bowman, the University of
  Glasgow, LabThings or OpenFlexure. `research/COLLABORATION_BRIEF.md` is an unsent draft.
- Do not modify LabThings or OpenFlexure source; no unsolicited upstream PRs.
- Learning must never widen permissions or change limits. Do not claim tamper-proofness.
- Do not declare production readiness because tests pass; the invariants in the prompt need new
  tests, not the old 225-run suite.
- Avoid unnecessary rebuilds and watch disk: **C: has about 2.5 GB free**; heavy artefacts live on D:.

## 8. Running things (Windows 11, Git Bash / PowerShell)

```sh
cargo test --workspace                       # 444 tests, a few minutes
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
python -m pytest python/tests -q             # 55 tests
python -m unittest adapters/python/wot/test_td_import.py   # 25 offline tests
```

**Gotcha:** cargo cannot overwrite `target/debug/openmhs.exe` while a gateway runs from it
("Access denied"). Stop the gateway, or copy `target/debug/deps/openmhs.exe` aside and point the
harness at the copy with `OPENMHS_BIN`. (Files named `deps/openmhs-<hash>.exe` are test binaries,
not the CLI.)

### Research environment (not in git; on this machine only)

`D:\openmhs-research` holds a venv with labthings-fastapi 0.3.0 and the OpenFlexure server
(v3 dev branch, simulation config, web app pulled with `pull_webapp.py`), the `labbench` example
Thing, gateway homes under `work/`, and logs. `D:\ollama-models` holds a private Ollama model
store (qwen2.5:3b). Servers were **stopped** at the end of the study; start them per
`research/EXPERIMENT_ENVIRONMENT.md`: OpenFlexure sim on :5000, labbench on :5100, hotplate
stand-in (`research/generalization/hotplate_server.py`) on :5200 with basic auth `lab`/`bench`,
private Ollama on :11435 with context 12288. **The maintainer's own Ollama on :11434 must not be
touched.** `D:\openmhs-release-target` is an old release-build target the maintainer may delete.

Re-running the study: `research/experiments/` (`reliability_benchmark.py`,
`provenance_experiments.py`, `multi_device_experiment.py`, `proposal_tests.py`,
`nl_experiments.py`, `async_reliability.py`), each documented in its header. Use `OPENMHS_BIN`
for the gateway binary. Local CPU models run about 5 tokens/s; cap `max_tokens`.

## 9. Known open items and risks

- The three P0 tool defects; audit gaps (receipt digests, keyed or anchored checkpoints,
  archive of descriptor bodies, per-person operator identity, software hashes).
- The OpenFlexure fixture `research/fixtures/openflexure-sim-thing-descriptions.json` (116 KB) is
  output of GPL-3.0 software: the maintainer must decide whether to ship, regenerate or drop it.
- The value model cannot express nullable or compound values (23 of 106 OpenFlexure properties).
- LabThings findings are against 0.3.0 only; newer versions unchecked.
- Natural-language question open: needs a capable model.
- Windows-only development so far; Linux and macOS release archives were never executed by the
  maintainer; one serial test is excluded on macOS (documented in `docs/limitations.md`).
- No Anthropic credentials on this machine; no hosted-model runs have been done.
