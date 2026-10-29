# A fault-injection reliability benchmark

Phase 8. The same work, the same LabThings server and the same faults, with different client
logic in front: plain HTTP versus the same work through OpenMHS. **The result is a trade, not a
win.** OpenMHS removed every duplicate execution and every out-of-limit dispatch, and a client
that obeys its receipts finished 20 of 45 runs against 35 of 45 for plain HTTP, because it stops
where it cannot be sure. Whether that trade is good depends on what a repeated or unconfirmed
action costs, which the benchmark cannot say.

Simulator only; one server; one action. Not a survey of devices, and not evidence about physical
hardware.

```sh
labthings-server -c labbench/config.json --port 5100
python research/experiments/reliability_benchmark.py --reps 5 > bench_main.json          # four arms
python research/experiments/reliability_benchmark.py --reps 5 --arms mhs-reconcile ...   # fifth arm
python research/experiments/summarize_benchmark.py bench_*.json > research/results/reliability_benchmark.json
```

Raw per-run data: [`results/reliability_benchmark.json`](results/reliability_benchmark.json)
(225 runs, 0 harness errors). Code: [`experiments/reliability_benchmark.py`](experiments/reliability_benchmark.py),
[`experiments/summarize_benchmark.py`](experiments/summarize_benchmark.py), shared fault proxy in
[`experiments/lab.py`](experiments/lab.py).

## What is compared

One intended command per run: *move the carriage to 100* (an action declared non-idempotent, 1.5 s
long). Ground truth is read from the LabThings server itself, after everything has settled: how
many times the work started, and where the carriage is.

| Arm | Client logic |
|---|---|
| `direct-naive` | HTTP straight to LabThings. If an attempt fails or cannot be confirmed, try again (3 attempts). What a first script, or an agent told to be persistent, does |
| `direct-careful` | HTTP straight to LabThings, written by someone who has read this repository's findings: never resend a possibly-executed action; after an ambiguous failure wait for the bench to be idle and read the position back; otherwise say "unknown" |
| `mhs-same-key` | Through OpenMHS; the naive retry loop with one idempotency key |
| `mhs-new-key` | Through OpenMHS; the naive loop with a fresh key each attempt (an agent that does not know a retry is a repeat) |
| `mhs-reconcile` | Through OpenMHS, a client that **heeds the receipt**: it does not resend work that failed part-way or ran unconfirmed; when an outcome is unresolved, an operator waits for the work to finish and asks OpenMHS to reconcile it by readback (applied → success; not applied → safe to send again) |

The two direct arms bracket plain HTTP from naive to careful. The three OpenMHS arms are not a
sequence of improvements; they show how much the *client's* behaviour matters on top of the
gateway.

| Fault | What it models |
|---|---|
| F0 | none |
| F1 | the response to the request is lost; the server acted |
| F2 | the connection is reset before the server sees the request (nothing ran) |
| F3 | the server answers 503 and did not act |
| F4 | the request is accepted; every status poll is then lost |
| F5 | the action fails part-way (stalled motor) |
| F6 | a cache serves the position from just before the move (a snapshot under a second old) |
| F6b | as F6, with a snapshot 2.5 s old |
| F7 | a target outside the declared limit (+/-1000); the bench itself has no limit |

Five repetitions each, 9 faults, 5 arms = 225 runs. Counts, not statistics: with n = 5 and
deterministic faults the point is which behaviours occur, not their frequency.

## Results

`S` completed (claimed success and the server agrees), `F` claimed failure, `U` reported unknown,
`d` runs in which the work started more than once, `fs` claimed success when it had not happened,
`ff` claimed failure when it had happened. Each cell is 5 runs.

| Fault | direct-naive | direct-careful | mhs-same-key | mhs-new-key | mhs-reconcile |
|---|---|---|---|---|---|
| F0 none | S5 | S5 | S5 | S5 | S5 |
| F1 response lost | S5 **d5** | S5 | U5 | U5 | S5 |
| F2 reset before send | S5 | U5 | U5 | U5 | S5 |
| F3 503, not run | S5 | S5 | F5 | S5 | F5 |
| F4 status polls lost | F5 **d5 ff5** | S5 | U5 | U5 | S5 |
| F5 fails part-way | F5 **d5** | F5 | F5 | F5 **d5** | F5 |
| F6 cache <1 s old | S5 | S5 | F2 U3 **ff2** | U5 **d5** | F3 U2 **ff3** |
| F6b cache 2.5 s old | S5 | S5 | U5 | U5 **d5** | U5 |
| F7 out-of-limit target | S5 (**ran it**) | S5 (**ran it**) | F5 (refused) | F5 (refused) | F5 (refused) |

| Arm | Runs | Completed | Runs with duplicate execution | False success | False failure | Reported unknown |
|---|---|---|---|---|---|---|
| direct-naive | 45 | 35 | **15** | 0 | 5 | 0 |
| direct-careful | 45 | 35 | 0 | 0 | 0 | 5 |
| mhs-same-key | 45 | 5 | 0 | 0 | 2 | 23 |
| mhs-new-key | 45 | 10 | **15** | 0 | 0 | 25 |
| mhs-reconcile | 45 | 20 | 0 | 0 | 3 | 7 |

The 35 completions of both direct arms include the 5 F7 runs in which an out-of-limit command was
carried out as asked.

### Reading the table

1. **Duplicates.** `direct-naive` executed the work more than once in 15 of 45 runs (F1, F4, F5):
   when the answer was lost it asked again. The other arms had none unless the client changed its idempotency key
   (`mhs-new-key`, 15 runs: see 5 and 7). In F1 `direct-naive` *looked* successful
   while running the job twice.
2. **OpenMHS's cost is completion.** The two naive OpenMHS clients finished 5 and 10 of 45 runs.
   They received honest "unknown" or "failed" receipts and had no way to act on them: in F1, F2 and
   F4 the work had either happened or provably had not, and only a reconcile could tell. The
   reconciling client finished 20 of 45 with no duplicate execution.
3. **Recovery was correct where it was attempted.** After lost contact (F1, F2, F4) the reconciling
   client got the right answer 15 of 15 times by readback: applied in F1 and F4, not applied in F2
   (then a safe resend).
4. **`direct-careful` is a strong baseline.** It finished as many runs as `direct-naive` with no
   duplicates, by waiting for the bench to be idle and reading the position back. It did what OpenMHS's
   reconcile does, in 49 lines of Python (below). What it lacks is everything outside the
   benchmark's scoring: no record, no limit (it ran the F7 target), no approval, and a position read
   that a cache would have fooled (see 6).
5. **F5 is not a win for anyone.** A part-way failure left the carriage at an unknown intermediate
   position in every arm. `mhs-new-key` ran the failing action three times and `direct-naive` three
   times; the other arms ran it once and said "failed".
6. **Freshness.** The direct arms never read a position in F6/F6b, so a stale cache could not mislead
   them here; this benchmark does not show an OpenMHS advantage on freshness. It shows OpenMHS's
   behaviour under a stale cache, which is the stated limit from the earlier study: with a snapshot
   under a second old (F6), the Date-header stamp cannot tell it from fresh data and OpenMHS
   *blamed a working device* (`ff` 2 and 3 of 5). With the snapshot 2.5 s old (F6b) it correctly reported
   "unverifiable" and never failed the device. A direct client that verified by reading the
   position would have been fooled by the cache in both; that was not measured.
7. **A new key defeats the duplicate protection.** In F6/F6b `mhs-new-key` retried after "ran, but
   unverified" and executed the work three times. Idempotency keys protect only a client that reuses them.
8. **Unauthorized dispatch.** The only measurable case here is F7: both direct arms carried out the
   out-of-limit command (the bench has no limit of its own), OpenMHS refused it before dispatch in
   15 of 15 runs. LabThings has no principals or grants to compare, so no other case is scored.

## Overhead

20 clean runs each (1.5 s action), no concurrent load, loopback, debug build of the gateway:

| | median | p95 |
|---|---|---|
| direct | 1.976 s | 2.014 s |
| through OpenMHS | 1.968 s | 2.016 s |

The two are indistinguishable at this resolution. The action's own 1.5 s dominates. This says the overhead is
small compared with a 1.5 s action; it says nothing about short actions, load or a real network. The
NL study measured a different cost: OpenMHS's *prompt* is 5-15 times the size of a direct tool
layer (NATURAL_LANGUAGE_EXPERIMENTS.md).

## Audit completeness

Across the 135 OpenMHS runs: the audit chain was valid in all 135 and a complete record (principal,
authorization decision, lifecycle) existed for every command. The device's own reference was
recorded in 85; it is absent when the response was lost before the invocation resource was seen
(mostly F1, F2 and F3) or when the request never reached the device (F7). The direct arms leave no record;
they print nothing and the harness read ground truth separately.

## Integration effort

Lines of Python in the benchmark for each client (excluding comments):

| | Lines |
|---|---|
| `direct-naive` (with its HTTP helpers) | 30 |
| `direct-careful` (with the helpers) | 49 |
| OpenMHS client loop | 34 |

For the OpenMHS arms the device's contract is generated from its Thing Description plus a reviewed
override file (WOT_IMPORTER_RESULTS.md); the `direct-careful` logic would have to be rewritten for
every action and every device. These are sizes of code in this file, not a measure of quality.

## What this does and does not show

Shown, in simulation, on one action and one server:

- Naive retry against a lossy LabThings connection duplicates work, and the duplicates look like
  success.
- A gateway with idempotency, an unresolved-outcome hold and readback reconcile removes the
  duplicates and the out-of-limit dispatch, at the price of completions a client must recover with
  an explicit reconcile.
- A careful plain client can match OpenMHS on duplicates for this action.
- The Date-header mitigation has a blind spot of about a second, and the effect is a wrongly
  blamed device.

Not shown:

- That OpenMHS improves agent *accuracy*. No model was involved here.
- Anything about real hardware, real networks (half-open connections, partial writes) or devices
  that lie.
- Rates. n = 5 per cell and deterministic faults.
- That the "reconciling client" is realistic: its operator knows to wait the right time and always
  acts. A person who does not reconcile leaves the system where `mhs-same-key` ends.
- A comparison with other supervisory systems. There is no baseline here other than hand-written
  HTTP.
