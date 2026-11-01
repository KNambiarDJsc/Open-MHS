# Blog notes: what building a safe execution layer for lab automation taught me

Raw material, not prose. Every claim has the place it came from, so you can check it before you
publish. Numbers are from one Windows desktop, simulated hardware only; say that early and say
it again near the end.

## Working titles

- "A simulator said yes to 400 µL in a 200 µL well": what lab-automation tools do and don't check
- "The answer got lost, not the pipetting": uncertain outcomes in lab automation
- "I put the same safety layer in front of five lab-automation stacks. Here is what I found."
- "Verified" is a word that needs a footnote

## The one-sentence thesis

Lab-automation software is good at doing things and mostly silent about what to do when it is
not sure whether a thing was done; an execution layer earns its keep at that edge, not on the
happy path.

## Opening hook options

1. **The doubled dose.** A lost network answer, a workflow retry, and a well that holds 120 µL
   instead of 60. The optimizer then sees the response of 120 µL (0.837) and believes it is the
   response of 60 µL (truth 0.646). Same loop, same simulator, with and without the layer.
   (`05_madsci`, MAD-04 vs MAD-04-native.)
2. **"The simulator accepted it."** Opentrons' simulator ran 400 µL into a 200 µL well, and a
   negative volume, without complaint. Micro-Manager's demo stage, asked for 10^9 µm, wandered off.
   (`02_opentrons` §7, `03_micromanager` §7.)
3. **What "VERIFIED" means.** In every one of these experiments it means "a simulator's own
   software state agrees". Walk through the eight levels from the brief (validated, authorized,
   dispatched, accepted, completed, state changed, independently measured, physically verified)
   and show how many are actually reachable in simulation.

## The story, as sections

### 1. The problem: tools that act, and tools that decide

- Agents and optimizers now sit on top of robot APIs. The robot API's job is to move; nobody's job
  is to say "this request was never allowed" or "I don't know if that happened".
- Define the layer in plain terms: a gateway between the thing that decides and the thing that
  moves. Contract before dispatch, an identity for every request, an honest "unknown", and a
  record you can verify without trusting the gateway.
- What it is not: not a safety system, not an orchestrator, not a replacement for any of the five.
  Hardware interlocks stay in hardware.

### 2. The experiment design (why it is more than five demos)

- Five ecosystems, one method: install pinned, run it natively first, then governed, then break it
  on purpose. (`00_ECOSYSTEM_ASSESSMENT.md`.)
- A **native baseline for every one**, so claims are against what the tool already does, not a
  straw man. (And say what's missing: the "careful custom wrapper" baseline was never built; admit
  that it might get most of the way.)
- Faults are the point: lost answer, killed gateway, two clients at once, device unavailable,
  edited protocol.
- Three records cross-checked in each scenario: what the orchestrator said, what the gateway said,
  what the simulator's own state says. Mismatch is a finding.
- Rules to quote: never claim a simulator proves physical safety; do not manufacture a success;
  report a blocked experiment as blocked.

### 3. Five ecosystems, one line each (and what each taught)

| | Taught |
|---|---|
| PyLabRobot | A transfer is the right unit, not a pipette movement. Don't expose low-level motion because you can |
| Opentrons | The unit is a *program*; you pin its bytes by hash and let the agent choose only numbers |
| Micro-Manager | One controller, many devices; a move-then-snap is only safe as a sequence under a lease |
| SiLA 2 | A standard description answers about half of what a safety layer needs; the rest must be reviewed by a person |
| MADSci | An orchestrator that fails a workflow on "unknown" needs something else to resolve it |

### 4. Findings that surprised me (each is a possible section or sidebar)

1. **Accepting too early turns a clean refusal into "maybe partial".** An adapter that said
   "accepted" before the device committed made a straightforward refusal look like a failure that
   might have applied. Fix: acknowledge only after commit. (`01` finding 1.)
2. **Reconciliation answers "is the target there now?", not "did this command happen?"**
   A move that did happen was reported "not applied" because a newer move had superseded it.
   Idempotent writes make this subtle. (`03` finding 2.)
3. **The unique-id problem.** To tell this request's effect from an earlier one, an adapter needs a
   fresh id per request. The conformance suite reuses one, so three adapters could not pass L2 for
   the same reason. A test suite and a safety property pulling in opposite directions. (`01`, `02`, `03`.)
4. **Reconciliation is only as good as the device's saved state.** Restart the simulator empty and
   an applied transfer reconciles as "not applied": a wrong answer nothing can detect. (`01` finding 4.)
5. **A lock held during a simulated hang froze unrelated reads.** Found by the conformance suite,
   fixed in three adapters. A good argument for fault-injection suites. (`03` finding 4.)
6. **The import that never returned.** Importing numpy inside a running adapter hung forever on
   Windows. Cause not fully understood; fix: import before the SDK starts reading stdin. Anyone
   writing a scientific-Python adapter will hit it. (`02` finding 1.)
7. **Descriptions are not contracts.** SiLA tells you types, units, ranges. It does not tell you a
   command is irreversible, idempotent, or what an error means for the instrument. Don't infer from
   names. (`04`.)
8. **"Concurrent" was weaker than it looked.** In the MADSci run, the orchestrator had already
   spaced the two experiments seconds apart, so the gateway's lock was never contended. Easy to
   write "OpenMHS serialized them" and be wrong. (`05` finding 4.)

### 5. What the numbers say (and what they don't)

- Overhead per governed operation: about 30–40 ms. Against a simulator that answers in ~1 ms that
  is a big ratio; against a robot or an orchestrator that polls every second, it vanishes. In the
  MADSci loop an iteration took 4.55 s with and without the layer.
- State the ratio honestly; don't hide it behind "only milliseconds".
- Complexity cost: a process, a manifest, pinned environments, ~300 lines per adapter.
  Honest answer to "when is it not worth it": one script, one bench instrument, one user.

### 6. A side thread: the storage engine that came out of the same weeks

If you want the "systems" half of the post. All from RFC 0002.
- The audit chain was re-hashed in full at every start: 7.3 s for a 1 GB journal.
- Three steps in sequence, with a measurement for each: a seal (7.3 → 2.4 s), then compaction
  (→ 80 ms, 1004 MB → 28 MB).
- **The exit test I wrote did not pass as stated.** "Start-up flat in history" was false after
  sealing alone; it only held after compaction. The doc says so. (Good example of measuring before
  claiming.)
- Compaction is the only milestone that deletes anything, so: off by default, behind a flag,
  verified against the rows first, a crash drill at every step.
- The identity of every command is kept forever (~150 bytes each) so a repeated idempotency key
  still returns the stored receipt and sends nothing.
- Honest limits: compaction holds the journal (37 s on 1 GB), segments are local, seals unsigned,
  older versions refuse a compacted journal.

### 7. Process lessons (the part other builders will relate to)

- Separate "my harness is wrong" from "I found something". I misread several harness bugs as
  findings before checking: a gateway that came back on a new port; two test stacks sharing an
  environment variable.
- Write what you did not test next to what you did: a dash in the matrix is a gap, not a pass.
  (`FINAL_RESEARCH_REPORT.md`: unauthorized access, stale observations and a SiLA restart were
  never exercised.)
- A test that fails on its own mistake is good news: the scanner for unclassified audit kinds
  flagged an error code at first.
- Read your own glue code with the same suspicion as upstream's: my MADSci node returns `failed`
  after 60 s of waiting for approval when it should return `unknown`. Found by reading, not testing.
- The machine's disk filling up during the work was its own small lesson in what "cannot write"
  does to a result file (two came out empty and were regenerated).

### 8. Where this goes next

- Physical validation: one low-risk device, supervised, with an independent measurement.
- An independent reproduction of these runs, ideally on Linux.
- Core changes, each needing a failing test first: preconditions over a request's own parameters;
  verifying an amount (or by command id) not an echoed id; a declared per-request-unique
  parameter; reconciling superseded idempotent writes; one reviewed-overrides format shared by the
  WoT and SiLA importers.
- A "reconciler" node: a small trusted component that resolves unknown outcomes for an
  orchestrator. Needs a trust discussion.
- Upstream conversations (nothing has been sent to anyone): MADSci's unknown-status handling,
  Opentrons' Python 3.11 import failure, SiLA's missing side-effect and idempotency information.
- Remote, autonomous labs: say plainly that this evidence does not justify them yet, and list what
  stands in the way (see `FINAL_RESEARCH_REPORT.md`, question 10).

## Lines worth quoting

- "A gateway that says 'I don't know' is more useful than one that says 'done'."
- "Verification is a claim about a specific observation. Say which one."
- "The simulator is not a safety check; it is a check that your program is well-formed."
- "A dash in the table is a gap, not a pass."

## Facts to re-check before publishing

- All numbers are from one Windows 11 desktop (i7-10700, NVMe), warm cache, release build of the
  gateway. A Raspberry Pi and a cold cache were not measured.
- Version pins: PyLabRobot 0.2.2, opentrons 10.1.0, pymmcore-plus 0.18.1 (MMCore 12.5.1), sila2
  0.14.0, madsci 0.8.1. They will have moved.
- Nothing was run on physical hardware. Nobody upstream has been contacted or has endorsed anything.
- Branch status: the lab-automation work is on `research/lab-automation`; the storage work is on
  `main`. Check which is merged when you publish links.

## Diagrams you could draw

1. The five-layer state ladder: planned → orchestrator → gateway → simulator → measurement.
2. The lost-answer timeline: submit → applied → answer lost → timeout → UNKNOWN → reconcile → retry
   (with and without the key).
3. Start-up time against journal size: 7.3 s / 2.4 s / 80 ms.
4. The property-by-ecosystem matrix with its honest gaps.
