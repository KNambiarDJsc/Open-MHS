# Model-driven stage survey runs (superseded design)

Two runs in which GPT-6 Luna moved the simulated stage itself through the OpenMHS MCP tools.
Interpretation: [../../../SAMPLE_SURVEY_WORKFLOW.md](../../../SAMPLE_SURVEY_WORKFLOW.md).

**S1's `truth` block is from before a scoring fix.** The simulator's invocation list omits each
move's input, so `move_inputs` is empty and `immediate_resends: 15` is an artefact, not 15
resends. The corrected figures, reconstructed from the trace's own `submit_command` steps (each
`VERIFIED`): 16 moves, 7 of the 9 grid points visited (missed (1,-1) and (-1,1)), no resend,
final position (0,0,0). S3 was scored after the fix (inputs read per invocation).

S3-direct was stopped mid-run when the experiment was redesigned; it produced no result.
