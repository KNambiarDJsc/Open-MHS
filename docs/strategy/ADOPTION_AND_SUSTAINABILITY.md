# Adoption, sustainability and strategic futures

How OpenMHS could come to have real users and maintainers, and which of three possible
futures is worth betting on. Written to be critical: the likeliest outcome for a project
like this is that nobody outside it ever uses it, and this document is about not letting
that happen by default.

**Evidence base.** Repository facts are from this audit (2026-10-28). Claims about other
projects marked **[repo]** come from [docs/research/landscape.md](../research/landscape.md),
dated 2026-10-23, and were not re-checked today. Claims marked **[unverified]** are from
general knowledge of those communities and must be checked before they are quoted anywhere.

## 1. Where the project stands

| Signal | Today |
|---|---|
| GitHub | 3 stars, 1 fork, 0 issues, 2 pull requests (both by the maintainer), Discussions off |
| Releases | None. No binary, no PyPI package, no crate, no docs site |
| Maintainers | One |
| Outside users | None known |
| Physical hardware validated | None |
| CI on `main` | Red on Linux and macOS since `37c56fb` (fixed on `strategy/availability`, not pushed); macOS runners queued for 6 h on four earlier runs |
| Engineering depth | High: 369 tests passing on Windows, a written specification, ADRs, a threat model, a conformance suite, a registry format |

The engineering is far ahead of the ecosystem. More engineering will not close that gap;
releases, a first physical validation, and a handful of real users will.

## 2. Positioning, honestly

| Alternative | What it does well | Where OpenMHS differs | Where OpenMHS does not |
|---|---|---|---|
| Anthropic's Model Hardware Standard | Announced 2026-08-27 as a limited research preview; specification not public **[repo]** | Independent, public, usable today | Cannot claim compatibility. If the standard is opened and wins, OpenMHS's best outcome is to become a conforming implementation |
| Other open "MHS" projects (Abenor-Labs, vraj130, huzjie, ≈20 on the awesome list) **[repo]** | Mostly Python, simulated-only, manifest → MCP tools → read/write/e-stop → audit | A durable journal, `UNKNOWN_OUTCOME` with readback reconciliation, idempotency, a conformance suite | None of them was inspected deeply; features may have moved since. Several are as unvalidated on hardware as OpenMHS |
| A generic MCP server per instrument | Trivial to start | One policy, journal and lifecycle across devices | Far lower effort for one device. The first user will compare against "a 50-line MCP server" |
| ROS 2 / ros2_control | The robotics standard; real-time control; huge ecosystem | Agent-facing authorization and evidence above it | Does not and should not do control. Positioned as a supervisory layer, it is complementary, not competing |
| OPC UA | The industrial interoperability standard | Agent policy and lifecycle | The information model is richer than a descriptor. OpenMHS should bridge, not duplicate |
| PyVISA, vendor SDKs | Direct, mature instrument access | A gateway and a contract in front | Directness. Researchers who write one script will not add a gateway |
| SiLA 2 | Standardized lab-device commands and observable commands **[repo]** | Cross-domain, agent policy | Real standard with manufacturer support; a bridge is the right relationship |

**What is genuinely different** (the only thing worth leading with): OpenMHS treats *an
uncertain physical outcome* as a first-class state and refuses to resend until it is
resolved, and it can show evidence for that. The other projects in the repo's landscape
research stop at allowed/blocked/returned. That is a real, narrow, defensible difference.
It is not "first", not "the standard", and not validated on hardware.

## 3. Who might actually use it

In rough order of how plausible adoption is:

1. **Researchers building agent-for-lab experiments** who need an agent to drive an
   instrument without the instrument being damaged or an experiment being silently
   repeated. They write Python, have one or two instruments, and care about reproducibility.
   They will adopt a tool that gets a first instrument working in an afternoon.
2. **Students and hobbyists** with ESP32 boards, Raspberry Pis and 3D printers: low stakes,
   high volume, the source of contributors and of word of mouth.
3. **Instrument and automation integrators** looking for a way to offer agent access to
   customers' equipment with an audit trail.
4. **Safety-conscious labs** (biology, chemistry) where "the agent did it twice" is costly.
5. **Manufacturers**: only after 1–4 exist. They adopt what their customers ask for.

## 4. Strategy, in the order that matters

### 4.1 Get one thing to work for one stranger (next 30 days)

1. Green CI on all three platforms; make macOS non-blocking until its runners are reliable.
2. Owner actions from `docs/release/M10_TEST_REPORT.md` (private vulnerability reporting,
   PyPI trusted publisher, Pages), then tag `v0.1.0` as a **draft** release, test the
   Linux and macOS archives from a clean machine, and publish.
3. `openmhs playground` ([PLAYGROUND_DESIGN.md](PLAYGROUND_DESIGN.md)): one command from
   install to an agent driving a simulated supply, with a visible refusal.
4. File the ten good-first issues already drafted in `docs/good-first-issues.md`; enable
   Discussions.
5. One short post showing the lost-acknowledgment demo (`apply-then-hang` →
   `UNKNOWN_OUTCOME` → blocked resend → reconcile) with the real terminal recording. It is
   the single most distinctive thing the project does, and it takes ninety seconds to show.

### 4.2 Get one physical result (30–90 days)

The ESP32 validation of [HARDWARE_VALIDATION_PLAN.md](HARDWARE_VALIDATION_PLAN.md), run on
a board with a multimeter, published with its evidence bundle, including anything that
failed. A project whose README says "no hardware has been tested" will be ignored by the
people it most wants; the first honest physical report changes that more than any feature.

### 4.3 Get one outside adapter (60–120 days)

An adapter in someone else's repository, built only from published packages, with a
conformance report in the registry. Before this, "third-party adapters" is a design, not a
fact. Offer to co-write it with a lab that owns an instrument OpenMHS has no profile for.

### 4.4 Get a second maintainer (90–180 days)

Bus factor one is the project's largest sustainability risk. Recruit for `adapters/` and
`registry/` first: low safety risk, easy to review, and the work that contributors already
want to do. Document what a second maintainer can merge alone.

### 4.5 Then research and pilots

Only after 4.1–4.3: a HardwareBench workshop paper ([HARDWAREBENCH_RESEARCH_PLAN.md](HARDWAREBENCH_RESEARCH_PLAN.md))
and one laboratory pilot with a written scope (one instrument, bounded operations, a
supervising person, a report at the end). Approach labs through the people who file
issues, not through cold outreach.

## 5. Ecosystem models worth learning from

| Model | Lesson | Status |
|---|---|---|
| ROS / ROS 2 | Decades of community process: enhancement proposals (REPs), working groups, a package index, coordinated releases | **[unverified]** details; the general model is well known |
| Hugging Face LeRobot | Low-cost hardware, simple robot/teleoperator classes, community contributions around real devices | **[unverified]** whether it now has an out-of-tree plugin mechanism; check before proposing an integration |
| MCP | A small protocol, many servers, a registry; adoption driven by hosts supporting it | **[unverified]** registry details |
| CNCF-style governance | Sandbox → incubating → graduated tiers, a technical oversight body, public criteria | General knowledge. Premature for one maintainer; useful later as a checklist |
| Hardware developer communities (Arduino, ESPHome, Home Assistant) | Device catalogs grow when adding a device is a config file, not code | Direct parallel to OpenMHS profiles; Home Assistant's integration model is in the repo's landscape table **[repo]** |

The recurring pattern: a project becomes an ecosystem when contributing *a device* is much
easier than contributing *to the core*. OpenMHS's JSON profiles for Modbus, MQTT, SCPI and
HTTP are the right shape for that and are its best asset for adoption. A profile for a
common bench instrument, validated by its owner, is worth more than a new engine feature.

## 6. Adoption signals to measure

Genuine use, not vanity. Reviewed monthly; none is a target to optimize alone.

| Signal | Why it matters | First threshold |
|---|---|---|
| Time to first success on a fresh machine (measured by a person who is not the author, from "install" to "an agent changed a simulated device") | The funnel's narrowest point | Under 15 minutes |
| Release downloads and PyPI installs, excluding CI | People trying it | Any non-zero number, then trend |
| Issues opened by people other than the maintainer | Someone cared enough to report | 5 |
| Pull requests merged from outside | Contribution works | 3, at least one not docs |
| Third-party adapters or profiles in the registry | The ecosystem premise | 1, then 5 |
| Physically validated registry rows, with evidence bundles | Technical credibility | 1 (ESP32), then 3 |
| Distinct organizations running it against their own equipment | Real use | 1 pilot |
| Independent reproductions of a published validation | Credibility of the evidence | 1 |
| Time-to-first-response on issues | Community health | Under 3 days |
| Second maintainer | Sustainability | 1 |

GitHub stars are not on the list. They help a first impression and say nothing about
use.

## 7. Sustainability

Honest options for sustaining a one-maintainer project:

- **Keep the scope small enough to maintain alone** while the maintainer has the time:
  prefer profiles and bridges over new engines, and say no to features without a user.
- **Share the cheap parts first** (adapters, registry, docs) before the safety-critical
  parts (engine, policy, spec), which stay under the maintainer's review per GOVERNANCE.md.
- **Academic funding** for HardwareBench is plausible (reproducibility and safety of
  agentic lab automation is a live topic) but is a bet on a paper, not a plan.
- **Sponsorship** (GitHub Sponsors, institutional) is unlikely to be meaningful before
  there are users.
- **Commercial support** is the only model that could pay for full-time work; see §8.

## 8. Three possible futures

Be critical about all three.

### 8.1 Open-source infrastructure: a shared execution layer used by other projects

| | |
|---|---|
| User problem | Anyone connecting an agent to hardware re-implements limits, audit, and the question of what to do after a timeout |
| Existing alternatives | Per-project MCP servers; ad-hoc scripts; ROS 2 and OPC UA for the layers below |
| Advantage | The lifecycle semantics are specified and tested; a conformance suite exists |
| Evidence today | The code and tests. No outside users. No adapter has met a physical device |
| Evidence required | Physical validation; one outside adapter; one other project depending on it |
| Technical risks | Specification churn before 1.0; no process sandbox for third-party adapters; unbounded audit growth ([STORAGE_AND_SCALING_RFC.md](STORAGE_AND_SCALING_RFC.md)) |
| Adoption barriers | No release; Rust toolchain needed to install; one maintainer; the name collides with at least six repositories **[repo]**; independence from a standard that may become dominant |
| Why it might fail | A larger project (or the MHS standard, once published) offers the same thing with more backing; or "a 50-line MCP server" is enough for most users |
| Build vs integrate | Integrate: ROS 2, OPC UA, SiLA 2, PyVISA, LeRobot. Build: lifecycle, policy, conformance |

### 8.2 Research infrastructure: HardwareBench

| | |
|---|---|
| User problem | No shared way to measure whether hardware-agent systems cause duplicate effects or report false success under faults |
| Existing alternatives | Self-reported test counts per project **[repo]**; general agent benchmarks that do not model physical uncertainty (to be verified in a literature search) |
| Advantage | Ground-truth delivery counters in simulators; a fault vocabulary and report format already exist |
| Evidence today | Conformance suite and negative controls (`broken.rs`). No benchmark yet |
| Evidence required | S01–S16 implemented; two other implementations run; a result that is not trivially predictable (OpenMHS should fail some scenarios first, e.g. stale readback, H4) |
| Technical risks | Simulator results do not predict physical behavior (RQ5); other projects may not expose enough to be scored |
| Adoption barriers | Other projects must care enough to be measured; reviewers will ask for physical evidence |
| Why it might fail | The result is "everyone passes the easy scenarios" or "no one else runs it". A benchmark nobody else uses is a test suite |
| Build vs integrate | Build the scenarios; reuse existing agent-eval harnesses for layer C |

This is the most credible *research* contribution and the cheapest to start. It is also the
one most dependent on being honest about what simulation can show.

### 8.3 Commercial infrastructure: supported private deployments

| | |
|---|---|
| User problem | A lab or integrator needs agent access to equipment with accountability, a supportable deployment and evidence for auditors |
| Existing alternatives | In-house scripts; vendor automation software; generic integration consultancies; MHS, if it becomes available |
| Advantage | Apache-2.0 core; audit and lifecycle design; possible conformance-testing service |
| Evidence today | **None.** No customer conversations, no pilot, no price anyone has agreed to |
| Evidence required | At least three organizations that run it on their own equipment and say what they would pay for; a concrete budget line |
| Technical risks | Everything in 8.1, plus fleet management, signed audit export, process sandboxing, high availability, none built |
| Adoption barriers | Buyers want references, support contracts and liability clarity that a one-person pre-alpha cannot give |
| Why it might fail | There may be no paying market for "safe agent access to instruments" separate from what instrument vendors will ship themselves |
| Build vs integrate | Compatibility testing and support are services; do not build a hosted multi-tenant platform |

No market-size figure is given because none is evidenced. The honest statement is: there
is a plausible niche in regulated or high-cost-of-error labs, and no evidence yet that
anyone would pay.

### 8.4 Ordering

1. Open-source infrastructure is the prerequisite for both others. Everything in §4.1–4.3
   serves it.
2. Research infrastructure is the best near-term differentiator and the most likely to
   attract maintainers and collaborators.
3. Commercial infrastructure should be revisited only after a pilot produces a user who
   asks for something the project does not give for free.

## 9. What would change this plan

- If the Model Hardware Standard is published: read it, then decide whether OpenMHS
  becomes an implementation. The independence rule (ADR-0007) means no claim before then.
- If a pilot lab asks for something other than what is built (a different protocol, a
  workflow tool): follow the user.
- If no one outside the project engages within six months of a real release and a physical
  report: reduce scope to what the maintainer finds valuable on their own and say so in the
  README, rather than keep building toward users who are not there.
