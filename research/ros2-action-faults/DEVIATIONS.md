# Deviations from, and clarifications of, the pre-registration

The pre-registration (`PREREGISTRATION.md`, commit `9a29ea1d11108fb7f0d4ba1d333c7f01c8776564`) is not
edited. Everything that differs from it, or that it left ambiguous, is recorded here with the date
and whether it was known **before** the confirmatory run.

## Known before the confirmatory run

1. **Pilot.** One pilot (n = 30 per cell, base seed 1) was run only to debug the harness. No pilot number
   is used as a result. It exposed no harness fault. One pilot trial was traced by hand: an M1 client
   resent after the measurement channel read `[0, None, 0, 1, 0]` for an operation that had in fact
   happened. That is the channel's false-denial error working as modelled (analytic probability
   0.02 % per decision), not a bug.
2. **Exposure to F6 differs by arm.** F6 places the client crash at `dispatch + U(0.1, D + 1)`. An M0
   operation ends about `D + 0.1` after dispatch, so the crash is "triggered" only when it falls inside
   that window. An M1 operation lasts about 8.5 s longer (settle plus readings), so the crash always
   falls inside it. M0 and M1 variants therefore face different effective fault exposure under F6.
   Contrast C2 (B-M0 versus C-M0) is unaffected because both are M0. Contrast C3 includes F6 and
   compares C-M0 with C-M1, so its F6 part is confounded by exposure. This is reported, not corrected.
   `fault_triggered` is stored per trial and summarised per cell.
3. **Operationalising the H3m wrong-resolution criterion.** The pre-registration says the wrong-resolution
   rate must not "exceed the Clopper-Pearson upper bound for α". This is read as: the criterion is
   violated in a cell if the **lower** 95 % Clopper-Pearson limit of the wrong-resolution rate among
   resolved M1 trials is above α = 0.01. The posterior assumes a prior of 0.5 for "applied"; the true
   base rate differs by fault (for example, under F5 the first goal never has an effect), so the
   α bound is not guaranteed per cell even in simulation. The rates are reported as observed.
4. **Bench-schema export** was listed as an output in the report but is not produced in this
   iteration. Results are `summary.json`, `run_record.json` and `trials.jsonl.gz` (one record per
   trial, including the typed labels and the ledger-derived effect count).
5. **Arm D, F8, real `rclpy`, the sensitivity sweep over channel error** are out of scope for this
   layer, as the pre-registration states.
6. **C-M1 verification of every success.** Both M1 arms take a measurement after every reported
   SUCCEEDED, as the pre-registration says. This costs 8.5 s per operation in the model and is why M1
   operations are longer.

## Found after the confirmatory run

Recorded 2026-11-03, after the confirmatory run.

7. **The confirmatory run was executed twice.** The first run wrote a gzip whose header carried a
   timestamp, so the file hash differed between runs. `run_study.py` was changed to write a
   constant header (commit `bd7fd18`) and the run was repeated from a clean tree with the same seed.
   The metrics (`summary.json`) were identical to the first run; only the file hash differs. The
   committed results are from the second run. Trial-record bytes are identical across repeated runs.
8. **H3m is falsified as pre-registered, in F6.** I predicted that careful clients without measurement
   would leave unresolved outcomes after a client crash. They do not (0 %): after a short client outage
   the result is still retained on the server, so the durable client re-reads it. This is a wrong
   prediction, kept in the record.
9. **The wrong-resolution count excludes duplicates caused by a wrong intermediate decision.** It counts
   final reports only. One duplicate (F1, measured arms, one seed shared by B-M1 and C-M1) arose from a
   run of false-negative readings that made the client resend. It appears under D, not under
   wrong-resolution. This was not anticipated by the pre-registration.
10. **Paired arms share seeds, including the measurement stream.** B-M1 and C-M1 therefore reproduce the
    same channel-error events, so their error counts are not independent evidence. Cells are
    independent across seeds, not across these two arms.
11. **Python version.** The harness was run only on Python 3.13.4.
