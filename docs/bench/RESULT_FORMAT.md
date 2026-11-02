# OpenMHS Bench: a proposed result format and how others could take part

| | |
|---|---|
| Status | **Proposal.** Nothing here is a benchmark yet: no rankings, no scores, one system under test |
| Schema | [bench-result.schema.json](bench-result.schema.json) (version `0.1`, expected to change) |
| Example | [example-result.json](example-result.json), converted from a real gap-suite run |

## What it would be

An execution benchmark for physical AI asks whether a system *behaves correctly when things go wrong
around a physical action*, not whether a model picks the right action. A lost answer. A killed
controller. Two clients at once. An approval that comes too late. For each, there is a claim that
can be checked by something other than the system under test: how many times did the device do it?

This is the part of the existing experiments that is not about OpenMHS. Its evidence already has
this shape (`research/lab-automation/06_contract_gaps/`), so the format only names what is there.

## The format

One file per run of one scenario on one system. The fields, and where they come from today:

| Field | Meaning | Existing source |
|---|---|---|
| `system` | System under test: name, version, source commit | `openmhs --version`, `run_record.source()` |
| `scenario` | Id and **version**, so a changed scenario is a different test | `run_gaps.py` |
| `target` | Simulator or device, its version, and `kind: simulator \| physical` | experiment manifest |
| `operation` | What was asked for, as data | request in the receipt |
| `fault` | What was injected, and where. `null` for none | the scenario |
| `invariant` | The expected property, in a sentence, with a machine-readable id | check descriptions |
| `transitions` | The states the system reported, in order | receipt `transitions` |
| `dispatch_count` | How many times the **device** received it, observed at the device. `null` if not observable | conformance `deliveries`, simulator counters |
| `recovery` | What recovery concluded, and whether that matches the device | reconcile receipt vs simulator |
| `evidence` | Files, with SHA-256, that support this result | exported audit, raw results |
| `outcome` | `PASS`, `FAIL` or `NOT_TESTED`, with `reason` required for the last | `run_record.py` |
| `environment` | OS, versions, packages | `run_record.environment()` |

**The three rules that matter most:**

1. **`dispatch_count` is measured at the device, not reported by the system under test.** A
   system that claims one dispatch and caused two must fail. If it cannot be observed, the field is
   `null` and the invariant depending on it is `NOT_TESTED`. It never defaults to 1.
2. **`target.kind` is required and has no default.** A simulated result is never presented as a
   physical one, including in any table built from these files.
3. **`NOT_TESTED` is an outcome, and it carries a reason.** Aggregations must show it and must not
   count it as a pass, a fail or a percentage of anything.

## Taking part without using OpenMHS

A third-party system needs no OpenMHS code, no OpenMHS adapter and no OpenMHS concepts:

1. **A target it does not control.** The benchmark provides a small reference device behind a plain
   interface (HTTP, and possibly a local socket) with a fault-injection channel the system under test
   cannot reach, and an independent counter of what it received. `research/lab-automation/` has the
   ingredients, tied to adapters today. Separating them is the first real work.
2. **A driver.** The participant writes a thin driver with three calls: `submit(operation) ->
   handle`, `status(handle) -> state`, and optionally `resolve(handle)`. Its native states map to a
   small vocabulary: `done`, `refused`, `failed`, `unknown`, `in_progress`.
3. **Run and publish.** The harness injects faults, compares what the system reported with the
   target's counter, and writes these files. The participant publishes them as they came out.

**Concrete invariants need no vocabulary beyond that.** "After a lost answer and a retry, the device
received it once" holds for a workflow engine, a robot SDK with its own retry, a hand-written script
or OpenMHS alike. "Reports `unknown` rather than `done` when no answer came back" is what a
system that lacks an `unknown` state fails, and that is a true, useful result, not unfairness.

**What a system does not have is `NOT_TESTED`, not `FAIL`.** A system with no approval concept does
not fail the approval scenarios. They do not apply to it, and the result says so.

**Conflict of interest.** OpenMHS's authors wrote these scenarios and OpenMHS passes them, so this
is not yet independent. The scenarios are derived from one system's failure model. Before any
comparison means anything, someone else must review the scenarios, and run them.

## What is deliberately absent

- **Rankings, and numerical safety scores.** A score implies the scenarios are representative and
  weighted, and neither has been established. Passing these says a system behaves correctly in these
  scenarios on this target. It does not say the system is safe.
- **Latency as a headline.** It can be recorded, but a fast system that double-dispatches is not
  better than a slow one that does not. It belongs in an appendix.
- **A leaderboard, a website, a certification.** None of this before the method has been validated.

## Before calling it v0.1

- [ ] A reference target separated from any adapter.
- [ ] A driver for one system that is not OpenMHS, written by reading its docs, with its own native
      baseline. The MADSci and native PyLabRobot baselines are the obvious first ones.
- [ ] An outside review of the scenario list: what is missing, and what is biased towards OpenMHS's design?
- [ ] A physical target for at least one scenario, supervised, to find what simulation hid.
