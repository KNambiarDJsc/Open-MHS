# Is any of this publishable?

Phase 11. An honest assessment of what the evidence could support, written for the maintainer.
**Not yet as a research claim. As a reproducible technical report or experience report, yes, and
with more work as a short workshop paper.** I have not surveyed the literature, so "novel" below
means "not found in the LabThings and OpenFlexure material I read", not "new to the field".

## What there is

| Contribution | Evidence | Strength | Novelty (as far as I can tell) |
|---|---|---|---|
| **A catalogue of what happens to a supervising gateway when a device's asynchronous protocol fails** (acceptance versus completion versus verification, six ways to lose contact after acceptance, cancellation that is only a request, duplicate behaviour) | the scenarios in ASYNC_ACTION_RELIABILITY.md with ground truth read from the server, plus a 225-run benchmark | Moderate: strong ground truth, one server, one action, simulated faults | Low to moderate: the problems are known in distributed systems; the concrete LabThings results and the reproducible harness are the contribution |
| **The stale-reading hazard for timestamp-less property reads** and a mitigation with a stated blind spot | S12 family; F6/F6b | Moderate: clear mechanism, measured effect and limit | Possibly useful to anyone doing verified writes over HTTP property reads |
| **A fail-closed Thing Description importer**, with measured coverage on a real description set | 14 Things, 95 properties, 50 actions: 0 of 50 declare `safe` or `idempotent`, 0 units; a second, independently written description | Moderate for the measurement, weak for generality (n = 2 device families, one authored by me) | The measurement of what real LabThings descriptions leave out is the interesting part |
| **Audit and provenance properties** and the limits of a hash chain | P1-P9 | Moderate as an engineering report | Low: tamper-evidence limits are well understood; the specific gaps found and fixed are useful |
| **Multi-device supervised workflow with a fault** | R1-R3, D1-D3 | Weak: one made-up workflow | Low |
| **Natural-language agents on an unfamiliar device** | 32 runs of a 3B model | **Too weak to support any claim.** The model could not use the tools; no accuracy-improvement claim is possible | None |

## What it would take

A reviewer would, correctly, ask for:

1. **A literature and prior-art survey.** Not done. Relevant ground is likely to include device
   abstraction layers, lab-automation middleware, idempotency and saga patterns, and agent tool
   safety. Until it is done no novelty claim should be made.
2. **More than one device family from outside the authors.** Two so far, and the second was
   written by the person who wrote the importer. The thing to measure is a set of real third-party
   descriptions.
3. **Physical validation, reported separately.** Everything here is a simulator. A simulator
   result and a hardware result are different claims; mixing them would be misleading, and no
   hardware claim of any kind can be made from this work.
4. **A capable model for the agent question.** The experiment as run answers a question about a 3B
   model, not about agents. Hosted or larger local models, with more than three near-identical
   repeats, and a scoring rubric agreed before looking.
5. **Statistics.** n = 5 per cell with deterministic faults shows which behaviours occur, not
   how often. Randomised fault timing, more repetitions and intervals would be needed for rates.
6. **Independent reproduction.** Everything ran on one Windows machine, a debug build of the
   gateway. Someone else running `reliability_benchmark.py` against a fresh LabThings install would
   settle whether the harness has quirks.
7. **A fair baseline.** The direct client I wrote (`direct-careful`) matches OpenMHS on duplicates
   with 49 lines. That is an honest and interesting result, but a paper would need to argue what
   the extra machinery buys (records, limits, approvals, a contract) rather than imply a reliability
   win that the data do not show.

## Realistic outputs, in order of effort

1. **A technical report** (arXiv-style, or the repository's `research/` directory as it stands),
   describing the harness, the findings and the negative results. Ready after editing.
2. **A short workshop or experience-report paper** on "supervising asynchronous lab-device actions:
   what LabThings leaves to the integrator", needing items 1, 2 and 6 above and a physical test
   by someone who owns the hardware. Venue and deadlines have not been checked.
3. **A software or resource paper** for the importer plus benchmark, once there are three or more
   real device descriptions and the project is past pre-alpha.

## Things that should not be done

- **Do not state or imply that Dr Bowman, the University of Glasgow, LabThings or OpenFlexure
  contributed to, reviewed, endorsed or are collaborating on this work.** Dr Bowman expressed
  interest in two features of OpenMHS. That is all that is established. Authorship or
  acknowledgement is for those people to agree, and only if a collaboration actually happens.
- **Do not claim hardware validation, compatibility with OpenFlexure's physical microscope, or any
  safety property.**
- **Do not publish a result that says OpenMHS improves agent accuracy.** It did not here.
- Do not submit anything upstream or contact any researcher without the maintainer's decision
  (UPSTREAM_OPPORTUNITIES.md, COLLABORATION_BRIEF.md).

## Licence and artefacts to check before releasing the research directory

- OpenMHS is Apache-2.0, LabThings-FastAPI is MIT, OpenFlexure is **GPL-3.0**. The integration
  talks to them over HTTP and copies none of their code.
- `research/fixtures/openflexure-sim-thing-descriptions.json` (116 KB) is output captured from the
  OpenFlexure server. Descriptions are data, but this is derived from GPL software's output;
  consider regenerating it in CI instead of shipping it, or ask. Flagged here rather than decided.
- Result files include transcripts of local-model runs. They contain nothing private but should
  be re-read before anything is published.
