# ADR-0013: Conformance suite and adapter scaffolding

- Status: Accepted
- Date: 2026-10-23
- Refines: [spec/conformance.md](../../spec/conformance.md) (levels defined in M1)

## Context

M1 defined conformance levels L0–L3 in prose. M5 makes them executable, so that an
adapter author can find out, from one `cargo test`, whether an integration tells the
truth about its device. That has to be true under faults as well as on the happy path.
The suite has to work for adapters the project has never seen, written by people who
have never read the engine.

Three things make this hard. First, most of what matters at L2 is about failure: lost
answers, garbled answers, refusals, retries. These cannot be observed by calling a
working device. Second, values, preconditions and pacing differ per device, and a suite
that needs per-device scripts would not get used. Third, the report has to be evidence a
reviewer can check, not a badge.

## Decision

**A library, run in-process.** `openmhs-conformance` exposes
`run(adapter, fixture, options) -> Report`. Adapter crates call it from an ordinary
`cargo test`, and `openmhs conformance` runs it against the built-in simulators.
L0 and L1 call the adapter directly, so the report shows exactly what the adapter
returned. L2 goes through a real `Engine` with an in-memory journal, acting as principal
`conformance:suite` under an allow-everything policy. The lifecycle states judged are the
engine's own, so the suite tests the adapter's answers, not a re-implementation of the
engine.

**A fixture is the fault boundary.** The adapter is tested together with whatever
stands where the device would be: a simulator, or a fake device speaking the real
protocol. The fixture injects five faults on request: `link_down`, `no_response`,
`lost_ack`, `garbled` and `refuse`. All except `link_down` affect only state-changing
requests, so precondition reads and readback keep working. The fixture also counts
deliveries, which is what makes the no-retry requirement testable. A fixture declares
which faults it supports; checks needing an unsupported one are `untested`.

**Nothing is assumed.** Every check ends `pass`, `fail`, `untested` or
`not_applicable`. A level is reached only when it and all levels below have no `fail`
and no `untested` checks, and at least one check passed at that level. An adapter tested
without a fixture reaches at most L0.

**One root cause, one failure.** A check that depends on another check that failed is
`untested`, and its message names that check. A device that does not implement one
command gets one `L2.command` failure, not five fault-test failures with advice that
does not apply. This was found by following the author's guide
([recorded run](../adapters/walkthrough.md)).

**Everything is derived from the descriptor.** Test values step from the current
reading: 5% of the range, at most half the slew limit, and at least four times the
verification tolerance. Booleans toggle; enums take another declared value. Failed
preconditions are remedied by writing the property, or by invoking an action whose
readback declares that it sets the property (for example `axis.home` for `axis.homed`).
Rate and slew windows are waited out. `Options::samples` overrides values when the
heuristics do not fit a device.

**Requirements have stable ids** (`L2.lost_ack`, ...). The registry in
`requirements.rs` and the table in `spec/conformance.md` must match exactly, and a test
enforces this.

**The report is evidence.** JSON, validated by
`spec/schemas/conformance-report.schema.json`. It names the subject by adapter id and
version, device id and the SHA-256 of the descriptor. It lists the `sources` the
readings carried, so a simulator result cannot pass as hardware. Each check carries its
message, its duration and, where relevant, receipts, readings or delivery counts.

**The declared level is a claim in the descriptor.** It lives in
`adapter.conformance` (`"L0"`–`"L3"`, optional). The report's `passed` compares the
reached level with that claim. Keeping the claim in the descriptor means agents and
operators see it with the device, and a test fails when the claim stops being true.

**Scaffolding from a tested template.** `templates/adapter` is a real workspace member,
built, linted and conformance-tested (in memory and over TCP) with everything else.
`openmhs adapter new` embeds its files, replaces three names and writes a standalone
`Cargo.toml`, so what it generates is known to work. The template adapter is
deliberately small: a lamp with a line protocol over TCP, which is how many bench
instruments (SCPI) speak. It shows every failure class (`Down` / `Lost` / `Timeout` on
the link, `ERR` refusals, unreadable replies), and includes a non-idempotent action with
a precondition and `verification: none`.

## Deviations from the M1 level text

- **`L1.subscribe` is `not_applicable`.** The adapter contract (v0.1) has no subscription
  API, so subscriptions cannot be tested. It becomes a real check when the contract gains
  one.
- **L3 cannot be reached.** `L3.leases` is always `untested` until M9 defines plans and
  leases.
- **Crashes are not a fixture fault.** The M1 text lists crashes among the faults each
  level covers. In-process adapters share the gateway's process, and a panic inside
  `execute` is contained by the engine itself: it ends `UNKNOWN_OUTCOME` with
  `adapter.internal`, and later commands still work. That is an engine property, tested
  once in `crates/openmhs-engine/tests/scenarios.rs` (`adapter_panic_is_contained`),
  not per adapter. Process crashes become testable per adapter in M7, with the
  out-of-process host.

## Alternatives considered

- **A black-box tester over the gateway API.** It could not see what the adapter
  returned, only what the engine made of it, so failure messages could not say what to
  fix. It would also need a running gateway per test.
- **Fault injection inside the adapter**, through hooks the author adds. This tests the
  hooks, not the adapter's real parsing and error mapping. A fake device exercises the
  code that will run against hardware.
- **Per-device test scripts.** More precise, but nobody writes them. Descriptor-derived
  values, with `samples` as an escape hatch, cover the three simulators and the template
  with no per-device code.
- **A code generator with blanks to fill in**, instead of a working template. Generated
  skeletons do not compile until finished, so the author's first experience is errors.
  A working adapter that passes L2 on the first `cargo test` gives a known-good
  starting point to change one step at a time.
- **Treating `untested` as neutral.** It would let an adapter reach L2 against a fixture
  that cannot inject faults, which is exactly the case conformance exists to catch.

## Consequences

- An adapter's claimed level is checked by its own test suite on every build.
- The suite really sends commands, including hazardous ones by default. It is for
  fixtures and simulators. The guide and the CLI say so, and `Options::hazardous = false`
  leaves those checks untested.
- The template and the scaffolder must stay in step. A unit test compares the generated
  manifest's dependencies with the template's, and the template is built and tested in CI.
- `openmhs adapter new` without `--openmhs-path` takes OpenMHS from the repository's `main`
  branch. That works only once this milestone is merged there, and it is not a pinned
  release; published crates would replace it.
- A conformance level is not a safety certification, and every surface that shows one
  says so.
