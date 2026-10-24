# Experiment 01: results

Run on Windows 11 x86_64, 2026-10-25, Python 3.11, OctoPrint 1.11.8 with its Virtual Printer
plugin, an `openmhs` debug build of this branch. See the [README](README.md) for what the
experiment is and how to rerun it. Raw evidence is in [`evidence/`](evidence):

| File | What it is |
|---|---|
| [`demo-transcript.txt`](evidence/demo-transcript.txt) | Output of `demo.py`, eleven steps, secrets redacted |
| [`wire-log.json`](evidence/wire-log.json) | Every request the adapter sent to OctoPrint during that demo, as seen by the proxy |
| [`test-run.txt`](evidence/test-run.txt) | `python -W error::ResourceWarning -m unittest discover -s adapters/python/octoprint -v` |
| [`../../../conformance/reports/octoprint.virtual-01/`](../../../conformance/reports/octoprint.virtual-01) | The conformance report |

> **Simulated vs physically verified.** Every result below is *simulated*: OctoPrint is real,
> the printer is its Virtual Printer plugin. Nothing is *physically verified*.

## Summary

| Check | Result |
|---|---|
| Real OctoPrint 1.11.8 installed in an isolated virtual environment from pinned, two-week-old packages | `pip check` clean |
| Read-only adapter written over OctoPrint's HTTP API, standard library only | 5 capabilities, 2 request paths, GET only |
| `openmhs adapter check` | ok |
| Adapter tests | 42 run, **41 passed**, 1 skipped (the abstract base class); 0 failed |
| Conformance (`openmhs conformance --adapter-manifest`, fixture mode) | **L1 reached, declared L1: PASS**, 34 pass, 0 fail, 0 untested, 6 not applicable |
| End-to-end demo | 11 steps, exit 0, 17 requests, methods `['GET']` |
| Rust workspace regression (`cargo test --workspace --no-fail-fast`) | see [Regression](#regression-and-security-tests) |

## What the tests cover

| Group | Tests | Needs |
|---|---|---|
| Mapping of recorded OctoPrint answers to capabilities, states, malformed and not-yet-reported temperatures | 7 | nothing |
| Configuration: read-only descriptors, undeclared endpoints, plain HTTP off loopback, missing credential, unknown keys | 7 | nothing |
| Read-only by construction: the client refuses any other path; the source has no write verb | 3 | nothing |
| Fault classification against replayed answers: 13 faults on 2 read paths, health, recovery, "no fault writes", the fixture fault, wrong key, closed printer, unreachable server | 8 | nothing |
| Real OctoPrint: version, discovery, every capability typed and fresh, wrong key, disconnect and recovery, faults never change the printer, wire shows only reads, key never in results | 9 | OctoPrint |
| Through a real gateway: discovery, no grant means no request reaches OctoPrint, granted reads, refused write, write grant cannot make a read-only device writable, disconnected printer, unreachable OctoPrint, audit chain and no secret in the gateway home | 8 | OctoPrint, `openmhs` |

The same fault table runs twice: against the replayed answers and against the real OctoPrint.

## Findings

Things the real OctoPrint showed that a reading of its documentation did not:

1. **Right after the printer connects, `GET /api/printer` has no `tool0` or `bed` entry at all**
   (only `history`), and a few seconds later has them with `actual: null` before real numbers.
   The first adapter classified that as `malformed_response`. It is now `device_failure`
   ("not yet reported"), the tests wait for the first poll, and the answer was recorded as a
   fixture (`printer-just-connected.json`).
2. **`/api/connection` has no port while the connection is closed**, so the adapter first labelled
   the offline `printer.state` reading `source: device` for a virtual printer. It now falls back
   to the configured port preference, so an offline virtual printer is still labelled
   `simulator`.
3. **An unplugged printer is a `409` on `/api/printer` but a `200` on `/api/connection`.** The
   adapter reads the connection first, so `printer.state` answers `offline` (quality `GOOD`) while
   temperature reads fail with `adapter.disconnected`: the gateway never serves a stale value.
4. **While OctoPrint is starting, a request can get a reply that is not HTTP at all**
   (`BadStatusLine: Not found`) before the server is ready. The harness treats that as "not up
   yet". Separately, the adapter and the harness ignore proxy environment variables and system
   proxies, as a precaution: a proxy would see the API key.
5. **On Windows a connection to a closed port can take longer than the adapter's timeout**, so
   "unreachable" is `disconnected` or `timeout` depending on the platform. Both are treated as
   the link being down by `health`.

## Deterministic failures and uncertain outcomes

For a command, an adapter error may mean *unknown outcome* (`adapter.timeout`,
`adapter.disconnected`, `adapter.malformed_response` after the request was sent) or *known: nothing
was sent* (`adapter.not_sent`, a gateway denial). This adapter has no commands, so every failure
is of the second kind for the printer's state, and the tests show it rather than assert it:

| Failure | Where it stops | Printer touched? |
|---|---|---|
| No grant | Gateway policy, `DENIED`, "nothing sent to the device" | No request reached OctoPrint (proxy count unchanged) |
| Write grant, but the capability has no `write` | Gateway validation, `command.operation.unsupported` | No request; OctoPrint's target still 0 |
| Printer unplugged | `adapter.disconnected` on temperature reads | Reads only |
| OctoPrint drops, stalls, errors, redirects, or sends garbage (13 variants) | `disconnected`, `timeout`, `device_failure` or `malformed_response`, each one class | After all 13: proxy methods = `{GET}`; OctoPrint targets still 0 and the printer still `Operational` |

What this does **not** test is the uncertain case itself (a write whose acknowledgement is lost,
`UNKNOWN_OUTCOME`, reconciliation). That path is tested against the in-repo simulators
(`openmhs sim fault sim.psu-01 apply-then-hang`, demo step 17), not against OctoPrint. Adding
writes to the adapter would be the next experiment, and would need its own safety review before
any physical printer.

## Regression and security tests

Rust and schema regressions were run after the last change to this branch's Rust-visible files
(there were none: this experiment touched no file under `crates/`, `spec/` or `Cargo.*`). The
workspace results are recorded in the [M10 test report](../../release/M10_TEST_REPORT.md).
The security-relevant tests of this experiment are: the credential reaching the adapter only by
name and never appearing in any result, error, audit record or gateway file; the refusal of
plain HTTP off loopback; the endpoint check; no proxy and no redirects; and the wire audit.

## Known limitations

* Simulated only. A real printer can differ in state names, temperature reporting rates and
  disconnect behaviour; OctoPrint's `Offline` versus `Closed` wording, for one, is mapped from
  text and is the likeliest thing to need adjustment.
* Read-only. No G-code, jobs, files or heater control. Only `tool0` and the bed.
* One OctoPrint version (1.11.8), one platform (Windows 11), Python 3.11 only. The Linux CI job
  is written but has not run, so the pinned requirements are untested on Linux.
* The conformance `link_down` fault is injected inside the adapter, not by pulling the
  printer; the integration tests cover the real disconnect.
* The temperature timestamp is OctoPrint's last sample; if OctoPrint's own poll stalls while the
  printer is still connected, the value ages until the gateway's 10 s freshness limit marks it
  unusable. This was not provoked.
* The API key is a long-lived global key in these tests; for real use prefer a
  scoped application key. The adapter checks neither scope nor key rotation.

## What this validates

That OpenMHS's out-of-process adapter model, permission manifest, credential passing, grant and
validation layers, error classification, freshness handling and audit log hold up against a
real third-party server and a hostile or flaky network, with no code changes to the core. It
says nothing about the safety of operating a printer.
