# RFC 0003: what the lab-automation research asks of the core

| | |
|---|---|
| Status | **Draft: 3 of 5 items settled without a contract change; 2 need a decision** |
| Evidence | [research/lab-automation/FINAL_RESEARCH_REPORT.md](../../research/lab-automation/FINAL_RESEARCH_REPORT.md), question 9 |
| Compatibility | Nothing proposed here removes or changes the meaning of anything released |

Five potential core changes came out of the simulated lab-automation experiments. This takes
each one in turn: the smallest case that fails, what was done, and what is left for a person to
decide. A proposal is here because it needs a new public field, or because its semantics are not
yet clear. None of them is implemented.

| # | Item | Outcome |
|---|---|---|
| 1 | Reconciling a superseded write | **Implemented** as a refusal. A third outcome is proposed below (§1) |
| 2 | Per-request-unique parameters | **Implemented without a new declaration** (§2) |
| 3 | Reviewed overrides, one format | **Compatibility note only** (§3): not worth a shared format yet |
| 4 | Preconditions over a request's own parameters | **Proposed** (§4) |
| 5 | Verification by amount or by command id | **Proposed** (§5) |

## 1. Reconciling a superseded write

**Smallest failing case.** `crates/openmhs-engine/tests/superseded_reconcile.rs`: an idempotent
write to 1 V is applied and its answer lost. A newer write to 2 V is allowed through, as
[lifecycle.md](../../spec/lifecycle.md) §3 rule 3 says. Reconciling the first read 2 V and said
`RECONCILED_NOT_APPLIED`, which is false. The reverse also happened: a write that never applied,
followed by a newer write of the same value, said `RECONCILED_APPLIED`.

**Done.** The dispatch chains `command.superseded` against unresolved earlier commands reading
the same state. Reconciling one is refused (`command.reconcile.superseded`), and it stays
`UNKNOWN_OUTCOME`. No state, schema or method changed; the API could already refuse to reconcile.

**Security.** The record is part of the audit chain and the check runs under the device lock, so
no write can slip between the check and a reading. It fails closed: if the record cannot be
written, the new command is not sent (`FAILED`, `adapter.not_sent`).

**What remains: the limitation.** `commands.resolve` can only say applied or not applied. For a
superseded command a person often cannot establish either, because the evidence was overwritten:
that is the whole point. They must pick one and explain in the reason. The record is honest about
the reasoning, and its state is not.

**Proposal.** A third terminal state, `RECONCILED_INDETERMINATE`, reachable only from
`UNKNOWN_OUTCOME` through `commands.resolve`, with a reason, meaning "this was examined, and what
it did cannot be established". Its `Outcome` would be neither `Succeeded` nor `Failed`.

**Why this is not done.** A new state breaks anything that matches exhaustively on states: the
Rust enum, `receipt.schema.json`, the Python SDK's `CommandState`, every client switching on it,
and evidence verified by older versions. It should ship with a contract version, not inside a
hardening branch. **Decision needed:** whether to accept that for a case that needs a lost answer
*and* a newer write, or to live with the forced choice and its recorded reason.

## 2. Per-request-unique parameters

**Smallest failing case.** PyLabRobot's `transfer_id`: readback compares `transfer.last_id` with
the request's id, so a reused id would verify against an earlier transfer, and the adapter rightly
refuses one. The conformance suite sent one fixed text value, so the adapter refused before the
injected fault could act, and three adapters stopped at L1.

**Done.** The proposal was a new `unique: true` on a parameter. It turned out not to be needed:
a parameter named by `verification.expect_param` is already declared to be how this command is
told apart from others, and `spec/conformance.md` §5 already says read-back parameters move from
their current value. The suite now sends text that is new on every command. PyLabRobot, Opentrons
and Micro-Manager reached L3 with no descriptor or protocol change.

**Compatibility.** No requirement id changed meaning. A text parameter read back by an adapter
that insists on one particular string would now fail, where before it passed. None of the
adapters in this repository does that; anyone relying on it can pass a sample value.

**Not done.** Nothing stops a *client* reusing such an id; the adapter's own refusal is still what
catches that. A declared `unique` could let the gateway refuse before dispatch, but that requires
it to remember values per capability, and adapters already do. No evidence yet that it is needed.

## 3. Reviewed overrides, one format

**The two formats.** The WoT importer (`adapters/python/wot/td_import.py`) and the SiLA 2 adapter
(`adapters/python/sila2/sila_adapter.py`) both take a person's reviewed statement of what a
machine description leaves out.

| | WoT | SiLA 2 |
|---|---|---|
| Keyed by | `actions.<name>` | `features.<feature>.commands.<command>` |
| Shared fields | `side_effect`, `idempotent`, `timing`, narrowed parameter ranges, `verification`, `preconditions` | the same |
| Only here | `unbounded_ok`, `reviewed_by` | `defined_errors_before_effect`, `freshness_ms` |
| Applied when | Generated once, then a manifest is reviewed | At each adapter start |

**Decision: do not unify yet.** The fields that matter are already the descriptor's own, with the
same names and meanings, so what a reviewer learns carries over. What differs is how each source
is keyed, which follows its standard, and the fields that exist only because of that standard. A
shared schema today would be a wrapper over two shapes, with two data points to design it from.
Both fail closed on anything unreviewed, and a test in each covers that.

**Recommendation.** Revisit with a third source of reviewed metadata. Meanwhile, the fields
in common must keep the descriptor's names (see [adapter quality gate](../adapters/QUALITY_GATE.md)).

## 4. Preconditions over a request's own parameters

**Smallest failing case.** "This well holds the requested volume." A precondition today compares a
property with a fixed value, so `liquid.transfer` can say "tips are available", but not "the source
named in this request holds at least the volume named in this request". In the PyLabRobot
experiment PyLabRobot's own tracker catches this at execution, so it ends `REJECTED` after being
dispatched, not `DENIED` before.

**Proposal (additive).** Two optional fields on a precondition:

```json
{ "id": "enough-stock",
  "capability": "source.{source}.volume",
  "op": "ge",
  "value_param": "volume",
  "max_age_ms": 2000 }
```

`value_param` takes the comparison value from the named parameter; `{source}` selects the property
from an enum-valued parameter.

**Why this is not done: the semantics are not settled.**
- *Reads chosen by a caller.* Today `state_needed(cap)` is static: reads are knowable from the
  descriptor. A templated name makes them depend on the request, so authorization has to be
  considered for a read the caller did not ask for by name. Restricting templates to `enum`
  parameters, and checking at registration that every expansion is a readable property, keeps that
  closed, but must be specified.
- *Units.* The comparison needs the parameter converted to the property's unit, and a failure to
  convert must deny.
- *A false sense of safety.* A precondition is checked before dispatch; a device with concurrent
  users can change between check and act. It narrows a window, and says nothing about an interlock.
- *Old gateways.* The descriptor schema refuses unknown fields, so a descriptor using these will not
  register on an older gateway. That is the correct failure, but it needs a descriptor version.

**Decision needed:** whether pre-dispatch refusal is worth this, given that the adapter's own check
already stops the effect, as it did in all observed cases.

## 5. Verification by amount or by command id

**Smallest failing case.** `VERIFIED` on `liquid.transfer` means "the adapter recorded this
`transfer_id` as the last transfer". Volume is not checked. A simulator that recorded the id and
moved the wrong volume would verify.

**Proposal (additive): two new verification forms.**

*By command id.* `{"mode": "readback", "capability": "last_command", "expect_command_id": true}`:
the expected value is the gateway's own command id, which the adapter already receives. It needs
no client-chosen id at all, and would remove the need for a reused-id rule in new adapters. It says
"this command was the last one recorded", no more than `expect_param` does today.

*By amount.* `{"mode": "delta", "capability": "dest.b1.volume", "expect_param": "volume",
"tolerance": {...}}`: read before dispatch, read after, compare the difference with the parameter.
This is the one that would actually check the physical quantity, on a device with an independent
sensor.

**Why this is not done.**
- A delta needs a reading taken before dispatch and recorded in the expectation, so
  reconciliation and restart recovery can use it. That reaches into the journal's bound
  expectation (`expectation_sha256`), which is the most safety-critical record in the system.
- Deltas and §1 interact: any other write to the same quantity invalidates the comparison, on a
  shared resource too.
- Nothing can test whether it is worth it in simulation. A simulated volume tracker agrees with
  itself by construction, so a delta verified in simulation would look stronger than it is. **This
  should wait for a physical device with an independent measurement.**

`expect_command_id` is small and could go first, but it adds a public field for a benefit no adapter
currently needs. **Decision needed** only if a new adapter asks for it.

## Summary of decisions needed from the maintainer

1. `RECONCILED_INDETERMINATE`: accept a contract version for it, or keep the forced choice (§1).
2. Preconditions over parameters: worth it, given adapters already catch this after dispatch (§4)?
3. Delta verification: agree it waits for a physical device (§5)?
