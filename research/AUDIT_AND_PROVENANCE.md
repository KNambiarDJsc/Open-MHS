# Audit and provenance

Phase 6. Dr Bowman's interest was in OpenMHS's audit trail. This phase asks what an investigator
could reconstruct afterwards about a command on a LabThings device, what damage to the record the
trail would notice, and what it would not. **The trail is tamper-evident for edits and deletions in
the middle, not tamper-proof, and it did not cover everything it appeared to.** Two gaps were
found and closed; several remain and are listed.

Simulators only. No claim is made about physical devices or about any safety or compliance
standard.

Data: [`results/provenance.json`](results/provenance.json). Code:
[`experiments/provenance_experiments.py`](experiments/provenance_experiments.py),
[`experiments/export_report.py`](experiments/export_report.py). Example exports:
[`results/reports/multi_device_R2_report.md`](results/reports/multi_device_R2_report.md) (a two-device plan with an
injected fault, approval, reconcile and a human decision) and
[`results/reports/provenance_P1_report.md`](results/reports/provenance_P1_report.md).

## What changed in OpenMHS because of this study

| Gap found | Change | Test |
|---|---|---|
| The device's own reference (the LabThings invocation id) was stored in the mutable receipt but **not in the hash-chained audit record**, so it was not covered by the chain | The `ACCEPTED` transition's detail now reads `device reference <ref>` | `crates/openmhs-engine/tests/provenance.rs` |
| `device.registered` recorded the adapter and counts but **not which capability contract** was in force | It now records `descriptor_sha256` of the validated descriptor in canonical form | same file |
| An imported device's contract did not say where it came from | The importer writes `x-wot-source` into the descriptor's extensions: the sha256 of the Thing Description imported, where it was fetched from, the importer version, and a hash of the reviewed overrides. It is inside the descriptor, so it is inside the hash | P1b below |

P1b (run by hand with the importer's command line against the hotplate description): importing the
same description twice gave the same recorded hash; adding one word to the description changed
it (`028d92ce…` → `a7c4890d…`). The recorded hash therefore says which description a command was
checked against, though not what the description *said*; see "Not in the trail".

## What the trail holds for one command (P1)

A command through the generated LabThings adapter, exported with `export_report.py`:

| Question | Answer in the trail |
|---|---|
| Who asked | principal `agent:exp` and credential id |
| What was asked | the full request: capability, operation, parameters |
| Was it allowed, by what | the decision, the grant that matched, the approver if one was needed |
| What happened and when | every lifecycle state with a timestamp: `CREATED … ACCEPTED … APPLIED … VERIFIED` |
| What did the device call this work | `labthings:<invocation id>` (and this id **was found on the LabThings server** and is in the chained record) |
| Was the result confirmed | expected value, observed value, observation time, source, quality |
| What contract was in force | `descriptor_sha256` at registration |
| Did anything go wrong | the device error text, if any |

Across the two-device plan (multi-device experiment), the same export also carried the plan, its
steps, and the **human decisions** on it (who, what, when, with a reason).

### Not in the trail

- **The contract itself.** Only its hash is recorded. The journal's tables are commands,
  approvals, leases, plans and the audit chain; the descriptor body lives in the adapter manifest
  on disk. To reconstruct what a check was against, the manifest (and the Thing Description) must
  be archived separately and matched by hash.
- **The raw HTTP exchange** between adapter and device. The adapter keeps its own JSONL ledger,
  outside the chain.
- **LabThings' own log** for the invocation. The reference makes it findable, not captured.
- **Which model, prompt or person produced a command.** Only the credential. The model's
  reasoning is not recorded.
- **A person's identity at the gateway.** Approvals and decisions are recorded as the *operator
  token* that made them (`operator:local`), not an individual.
- **Software identity.** The audit records a version string, not the binary's hash.
- **Wall-clock trust.** Timestamps are the gateway's; the device's clock is not recorded.

## What damage the trail notices (P2-P7)

Each experiment stops a gateway with real commands in its journal, damages the SQLite file the
way a local actor with write access could, restarts it and asks the gateway to verify its own chain.

| # | Damage | Result |
|---|---|---|
| P2 | Edit one chained record's data | **Detected**: first invalid record is exactly the edited one (seq 14) |
| P3 | Edit a record **and recompute every later hash** | **Not detected.** The chain has no secret in it; anyone who can write the file can rewrite it consistently |
| P4 | Delete one record from the middle | **Detected** at the record after the gap (seq 15) |
| P5 | Delete the newest 8 records | **Not detected by the chain alone**: it verified as valid, and the restarted gateway carried on appending to the shortened chain. **Detected against an externally kept anchor** (the head's sequence and hash recorded earlier): the anchor's record was missing |
| P6 | Edit the receipt row (the mutable per-command record) to say `FAILED` | **The chain reported valid**: the receipt is not covered by it. **Detected by the report's cross-check** of each receipt against the chained transition records |
| P7 | Kill the gateway mid-command | Chain valid after the kill; the command became `UNKNOWN_OUTCOME` and was **not completed or resent**; the device finished the move on its own (position 77) |

So: edits and deletions *inside* the chain are detectable, a rewrite by someone who can recompute
the hashes is not, and removing the newest records is not unless an anchor is held somewhere the
gateway's writer cannot reach. `export_report.py` prints the anchor and checks it on the next
export. The cross-check of receipts against the chain is new in this study.

## Storage exhaustion (P8)

The gateway was started with `--journal-max-mib 3 --journal-reserve-mib 1` and fed 0.1 s moves
until it refused:

- 448 commands were accepted and executed; the **LabThings server had started exactly 448 jobs**.
  No job ran without a command and a record.
- At 2 MiB (the start of the reserve) the next command was refused with `storage.exhausted` and
  the text "nothing was sent". **Reads were refused too**, because reads are audited (ADR-0021).
- The chain was valid afterwards.

This is the right ordering (record before dispatch, refuse when the record cannot be written).
The cost is that an agent that has filled the journal can neither act nor read, so monitoring
free space is an operational requirement. Only the reserved-writes side of ADR-0021 (stops and
recovery still allowed) was not exercised here.

## What LabThings forgets (P9)

OpenMHS holds the story; LabThings' copy is short-lived and its expiry is unusual:

- A completed invocation was still `200` **330 s** after it finished, although retention is 300 s.
- LabThings removes expired invocations **only when another action is invoked** (a background task
  after each action POST). After one further action the same id returned `404`
  ("No action invocation found with ID {id}": a literal `{id}`, see UPSTREAM_OPPORTUNITIES.md).
- OpenMHS still held the command, its `VERIFIED` state, the verification evidence and the
  reference, which no longer resolves on the server.

For an investigator this means: the LabThings side of a command cannot be relied on after a few
minutes, and how long it is available depends on traffic. The gateway's record is the durable one,
which is only as good as the protections above.

## What a reviewer should and should not conclude

Should: the trail now links a command to the device's own reference and to the exact contract it
was checked against, inside the chain; edits and deletions in the middle are detected; a short
export reconstructs who, what, when, under which authorization and with what evidence.

Should not: that the trail is tamper-proof, that it would stand as legal or regulatory evidence,
that a gateway that lied would be caught, or that it records physical truth. It records what the
devices *reported*.

## Recommended next steps (not built)

1. **Keep an anchor off the gateway.** Periodically publish the head hash (a file on another
   machine, a signed line in a log service). The export already supports comparing against one.
2. **Make the chain keyed.** An HMAC or signature with a key the writer does not hold defeats P3.
3. **Cover the receipt.** Either chain a digest of the receipt at each transition, or stop treating
   the receipt row as a source of truth. Until then, the export's cross-check is the control.
4. **Archive the contract.** Store the descriptor and Thing Description bodies (or require the
   manifest be archived) alongside the hash.
5. **Identify people.** Per-person operator credentials, so a decision is attributable to someone.
6. **Record the software.** The binary and adapter hashes at start.

## Limits of this study

One machine, SQLite files damaged by a script, a single local attacker model. It does not test
concurrent writers, a corrupted disk, backup and restore, clock changes, or a malicious adapter.
Counts are from single runs of deterministic damage, so they show behaviour, not rates.
