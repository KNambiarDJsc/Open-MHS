# A realistic imaging workflow on the OpenFlexure simulator

What an OpenFlexure user actually does is not "move to x = 1". They look for the sample, scan
around it, find the interesting regions, focus, take a closer look, and keep a record. This
experiment runs that workflow end to end on the **OpenFlexure simulator**, with OpenMHS governing
every stage movement and a language model kept away from the motors. **Simulator only: the images
are synthetic, the stage is a dummy stage, and nothing here is evidence about a physical
microscope.**

Full run report with every image, table and audit result:
[`results/reports/sample_survey/README.md`](results/reports/sample_survey/README.md).
Code: [`experiments/sample_survey_workflow.py`](experiments/sample_survey_workflow.py).

## Design

| Phase | Who acts | What happens |
|---|---|---|
| 1 Discover | workflow | Read the live Thing Descriptions (14 Things), import the stage into OpenMHS, check which numeric quantities declare a unit, ask OpenMHS which operations are allowed (`access.allowed_operations`), read the start position through OpenMHS |
| 2 Survey | workflow, **approved procedure** | A 5×5 serpentine raster, spacing 500 simulator steps (−1000..1000), its point list hashed and approved before execution. Principal `agent:survey` may move only within the reviewed envelope (x, y within ±1200; z within ±400). One simulated image is captured at every OpenMHS-verified position |
| 3 Analyse | deterministic code, **no model** | Per image: sample coverage (pixels darker than the empty background), number of distinct features (connected components), sharpness (Laplacian variance), contrast |
| 4 Propose | **language model** (GPT-6 Luna) | Sees only the measurement table, never an image. It has one tool, `propose_followup`, which records a proposal and moves nothing: up to 3 regions of interest with reasons, up to 4 follow-up points within 250 steps of a named region, and which region to refocus |
| Validate | deterministic code | Every proposed image id must exist; every number the model cites must be in that image's row; points must lie in the envelope, near their region, and not repeat a surveyed position |
| 5 Adapt | workflow, **each move approved** | Focus sweep at the chosen region (5 z positions through OpenMHS, sharpest kept), then the validated follow-ups. These moves run as a second principal, `agent:adaptive`, whose grants are `require_approval`; the operator (the harness) approved only validated proposals |
| 6 Fault | injected | At survey point 12 of 25, the status polls of an accepted move are dropped. The workflow must suspend, keep the evidence, reconcile from readback, and never resend |
| 7 Report | workflow | Procedure, actions, observations, image sha256s, timestamps, versions, OpenMHS audit export verified offline (`openmhs audit verify-file`), ground truth from the simulator, open uncertainties |

## Results (run 2026-10-30, OpenMHS `db950d7`, OpenFlexure server 3.0.0b1 simulation, LabThings-FastAPI 0.3.0)

- **Discovery:** 0 of 7 numeric stage/camera fields declare a unit, so all positions are recorded as
  simulator steps, never millimetres.
- **Survey:** 25 images. The sample sits around the origin. The centre tile (`img-s12`) has 82%
  coverage and sharpness 71; the tiles 500 steps to either side in x have 34–37%, those 500 steps away in y about 3%; the outer
  ring is empty background.
- **Model proposal:** the model ranked `img-s12`, `img-s11`, `img-s13`, the same three as the
  deterministic ranking. **All 9 numbers it cited exist in the table.** It proposed four points 250
  steps around the centre and a refocus there. Validation accepted all four; nothing was rejected,
  and there were no fabrication flags. One model call: 1,900 input and 226 output tokens,
  estimated **$0.0003**.
- **Focus sweep:** sharpness 3.1, 4.2, **71.2**, 4.7, 2.9 at z = −300, −150, 0, +150, +300; best z = 0.
- **Follow-ups:** four new images, each held for approval and approved after validation (15 approvals
  in total, including the return moves). One follow-up (`img-a0`, 250 steps above the centre) shows
  11 features, more than any survey tile.
- **Fault:** the move to `x = 0` at survey point 12 lost its status after acceptance. OpenMHS returned
  `UNKNOWN_OUTCOME`; the workflow suspended, waited, and reconciled: `RECONCILED_APPLIED`. **The
  simulator's own records show that move executed exactly once.**
- **Ground truth versus record:** the simulator executed 41 moves and OpenMHS recorded 41, every one
  with the simulator's invocation id; none ran without an OpenMHS record. The stage finished at its
  start (0, 0, 0).
- **Audit:** 376 records, chain valid; the exported evidence verified offline with
  `audit verify-file` exit 0 (valid and complete).

## Why the model is kept off the motors: an earlier comparison

Before this design, the same model was given OpenMHS's MCP tools directly and asked to run a 3×3
grid with a z-stack, moving the stage itself one axis at a time (traces in
[`results/reports/realistic_survey/`](results/reports/realistic_survey/), code in
[`experiments/realistic_survey.py`](experiments/realistic_survey.py)). Two runs:

| Run | What the stage did (simulator records) | What the model reported |
|---|---|---|
| S1, no fault | 16 moves, all `VERIFIED`; **visited 7 of the 9 grid points** (missed (1,−1) and (−1,1)); ended at (0,0,0) | A table of **all 9 points as "Confirmed"**, and that the stage ended at (1,1) and was **not** back at the start |
| S3, lost status on the 4th move | 16 moves; visited 9 of 9; the faulted move returned `UNKNOWN_OUTCOME`, the model called `reconcile_command` straight away (`RECONCILED_APPLIED`) and did not retry; ended at (0,0,0) | Accurate, including the fault and that it was not retried |

Every command in S1 was correctly verified by OpenMHS. The model's *summary* was still wrong in
both directions: it claimed points it never visited and denied a return it had made. The audit
trail, not the agent's narrative, was the reliable account. That is the argument for the design
above: the model proposes from deterministic measurements, deterministic code checks the proposal,
and OpenMHS executes and records. These two runs cost about $0.10 between them (about 500,000
input tokens each, because a model driving every move re-reads the whole conversation each turn),
against $0.0003 for the model step in the workflow above.

## What this shows and what it does not

Shows, in simulation: a complete discover–survey–analyse–propose–adapt–report workflow in which
every movement is bounded, authorized, verified and recorded; a lost acknowledgement mid-survey
handled without repeating the move; a model whose contribution is checked against data before
anything moves; and an evidence bundle that verifies offline.

Does not show:

- anything about a physical microscope, real samples, or real optics;
- that the image analysis is meaningful for real samples (its thresholds were chosen for this
  simulator);
- that OpenMHS attests image *content*: images are captured by the workflow as camera reads and
  linked to OpenMHS-verified positions by time, position and sha256;
- reliability rates: this is one complete run (plus one dry run without the model), not a
  statistical study;
- that the model would behave the same on other tasks; or anything about other models.

## Reproduce

```powershell
# OpenFlexure simulator on 127.0.0.1:5000 (see OPENFLEXURE_SIMULATOR_QUICKSTART.md)
$env:OPENMHS_BIN = "D:\openmhs-target\debug\openmhs.exe"
$env:OPENAI_API_KEY = "<your key>"          # or use --no-model for a run with no API calls
D:\openmhs-research\venv\Scripts\python.exe research\experiments\sample_survey_workflow.py
```

The script needs numpy, Pillow and scipy (the OpenFlexure server's environment has them). Spend
is capped by a ledger outside the repository; the key is read from the environment only.
