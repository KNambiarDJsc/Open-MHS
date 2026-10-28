# ADR-0022: Insights: learning from the journal, never from a back door

- Status: Accepted
- Date: 2026-10-28
- Builds on: [ADR-0002](0002-command-lifecycle.md), [ADR-0012](0012-mcp-server-and-sdk.md),
  [ADR-0017](0017-identity-and-trust.md)

## Context

An agent working with a physical system has to iterate: ask, observe what happened, adjust.
Within one conversation it already can: a refusal carries the numbers that bound it, and
`confirmed` says what readback showed. Across conversations it starts cold every time, and
the obvious fix, a place where the agent writes down what it learned, is the wrong one:

- Notes an agent wrote are text. Read back by a later agent they become a stored prompt
  injection (from a compromised device description, a hostile operator message, or the
  model's own earlier mistake) that survives restarts and looks like the agent's own
  knowledge.
- Notes cannot be checked. "The supply can do 20 V" may be false and nothing says so.
- A write path for learning is a new thing to bound, audit and keep out of the storage
  reserve (ADR-0021).

## Decision

**Learning is a read of what the gateway already recorded.** `Client::insights` (and the MCP
tool `get_insights`, and `openmhs insights`) lists the caller's recent commands and
summarizes them per device and capability:

- the final states reached (`VERIFIED`, `DENIED`, `UNKNOWN_OUTCOME`, ...);
- refusals by code, most frequent first, with the numbers from the latest one of each kind
  (the slew or range limit the agent ran into) and the id of the command they came from;
- the range of values readback confirmed, and the latest;
- how far confirmed readings landed from the request (tracking error), as maximum and mean;
- time from dispatch to confirmation (median and maximum);
- outcomes still unresolved, which must be reconciled before another write.

Every figure names the commands it came from. Nothing is copied from device or agent text:
only codes, numbers, states and ids. The computation is a pure function over receipts
(`openmhs_api::insights::summarize`), so it is reproducible from the journal, and it adds no
write path, table, API method or setting.

### Boundaries

- **It cannot widen access.** Insights is read-only and is built from commands the caller was
  already allowed to list (an agent sees its own; the operator sees everyone's). Limits,
  grants, scopes, descriptors and policy are unchanged by what it reports or by what an agent
  concludes from it. A model that decides a refusal was wrong still gets the same refusal.
- **It is evidence, not a guarantee.** A value confirmed yesterday is not a promise about
  today: the device, its load or its environment may differ. The server instructions say to
  keep steps small and reversible, and to stop and report when the evidence does not support
  a step. The next command is still checked live (`check_command`, then the full pipeline).
- **It is a bounded window.** The newest 500 commands the credential can list. `window`
  reports how many were examined and `truncated` says when older history may exist.
- **Readback is the only source of "what the device did".** A command that was only
  acknowledged, or that ended `UNVERIFIABLE`, contributes to counts but never to confirmed
  values or tracking error. A reading older than its command cannot confirm it (ADR-0020), so
  it cannot appear here either.
- Quantities are not mixed across units; the newest command's unit is used.

### What this does not do

It does not model the device, infer a control law, schedule experiments or tune anything.
That is the agent's job, and a harder one than a gateway should guess at. It gives the agent
the facts it would otherwise forget. Nor does it search across devices or retain more than
the journal already does (retention is the draft storage RFC).

## Alternatives considered

- **Agent-written notes / a "lab notebook".** Rejected for the reasons above. If operators
  later want shared, human-written commissioning notes, they belong in the device profile or
  a document the operator controls, not in a channel the agent writes and reads.
- **Server-side learning that adjusts limits.** Rejected outright: a limit that relaxes
  because an agent behaved is a safety boundary moved by the thing it bounds.
- **A new `insights.get` API method.** Deferred. Computing client-side from `commands.list`
  keeps the gateway's surface and security boundary unchanged. If the window proves too small
  for long-lived deployments, a server-side aggregate over the journal is the next step, with
  the same output and the same permission rules.

## Tests

`crates/openmhs-api/src/insights.rs` (refusal counting and ordering, confirmed values and
tracking error only from verified commands, unresolved outcomes, other devices ignored,
truncation reporting, units not mixed); `crates/openmhs-mcp/tests/mcp.rs`
(`insights_summarize_what_the_journal_recorded`: refusals with their numbers, a confirmed
write, an unchanged permission afterwards); `crates/openmhs-cli/tests/cli.rs`; and
`examples/agent-loop/learning_agent.py`, where a second session reaches its target without a
single refusal because it read the first session's evidence.
