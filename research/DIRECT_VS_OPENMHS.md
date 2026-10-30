# Direct LabThings execution versus OpenMHS

This is the short comparison for the OpenFlexure/LabThings discussion. The complete methods,
per-fault results, limitations and raw JSON are in
[RELIABILITY_BENCHMARK.md](RELIABILITY_BENCHMARK.md) and
[`results/reliability_benchmark.json`](results/reliability_benchmark.json).

## Result in one table

The benchmark ran 225 simulator trials: five repetitions for each of nine fault conditions
and five client strategies. Each trial requested one 1.5-second move on the independent
LabThings `LabBench` simulator. It recorded the server's job count and final position as
ground truth.

| Client | Completed | Duplicate executions | False success | What it shows |
|---|---:|---:|---:|---|
| Direct, naive retries | 35/45 | 15 | 0 | Retrying ambiguous POSTs can run the job more than once. |
| Direct, careful recovery | 35/45 | 0 | 0 | A hand-written client can match the gateway's completion and duplicate counts for this one action. |
| OpenMHS, same idempotency key but no reconciliation | 5/45 | 0 | 0 | Duplicate suppression alone leaves many outcomes unresolved. |
| OpenMHS, new key per retry | 10/45 | 15 | 0 | A caller that changes keys defeats retry deduplication. |
| OpenMHS, receipts plus reconciliation | 20/45 | 0 | 0 | The gateway prevents duplicate dispatch and out-of-limit work, at the cost of conservative stops and explicit recovery. |

These are deterministic behavior counts, not statistical estimates (`n=5` per cell). The
careful direct client matches OpenMHS on duplicate suppression for this benchmark. OpenMHS
adds a durable authorization/lifecycle record, enforced bounds, approval, and shared policy;
the experiment does not show that its agent interface is always more useful or that it
increases completion.

The direct client also executed the out-of-range simulator target in all five F7 runs,
because its LabThings endpoint had no such guard. That establishes the effect of the bound
in this setup; it is not a claim that LabThings itself should own every policy.

## Limits

One LabThings simulator, one action, one machine, one OS, small deterministic sample. The
OpenFlexure stage-specific quickstart is separate and reads the live OpenFlexure simulator.
Neither experiment used physical hardware. A careful direct client can be appropriate when
its simpler policy and audit needs meet the lab's requirements.
