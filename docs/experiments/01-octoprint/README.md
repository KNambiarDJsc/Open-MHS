# Experiment 01: an agent and a virtual 3D printer

Can OpenMHS be tested against a real external system without owning any hardware? This
experiment puts an OpenMHS gateway in front of [OctoPrint](https://octoprint.org/), the
common 3D-printer server, whose bundled **Virtual Printer** plugin behaves like a printer's
serial firmware. An agent reads the printer through the gateway; the printer is unplugged and
plugged back in; the network to OctoPrint is broken in several ways.

[Results](REPORT.md) · [Adapter reference](../../adapters/octoprint.md) · [Transcript](evidence/demo-transcript.txt)

> **Simulated, not physical.** OctoPrint is real software speaking its real HTTP API. The
> printer behind it is a plugin. Nothing here says anything about a physical printer, a serial
> link, firmware, heaters or motors. No model and no paid API are used.

## What it validates, and what it does not

| Validated (against a real OctoPrint 1.11.8) | Not validated |
|---|---|
| The adapter talks OctoPrint's documented HTTP API and maps real responses onto OpenMHS descriptors and readings | Any physical printer, any serial or USB link, real firmware |
| Discovery, grants, denials, audit and error reporting work through a gateway for a device the project did not write | Heater, motor or endstop behaviour; thermal runaway; anything safety-related |
| A denied or unsupported request never reaches OctoPrint (counted at a proxy) | Writes: the adapter is read-only, so write lifecycle against OctoPrint is untested |
| Disconnecting the virtual printer (`!!DEBUG:action_disconnect`) surfaces as `adapter.disconnected`, never as a stale `GOOD` value, and reads recover | Uncertain outcomes (`UNKNOWN_OUTCOME`) with OctoPrint: they arise only from writes, which this adapter cannot send |
| Network faults (drop, delay, 4xx/5xx, redirects, garbage, truncated and oversized bodies) get a stable error class | OctoPrint versions other than 1.11.8, other platforms than Windows 11 x86_64 (a Linux CI job exists but has not run) |
| The API key stays out of results, errors, the audit log and the gateway home | OctoPrint authentication other than the API key |

The existing write lifecycle (`VERIFIED`, `REJECTED`, `UNKNOWN_OUTCOME`) is exercised by the
other simulators and the conformance suite, not here; see
[the demo](../../tutorial.md) and [simulators](../../simulators.md).

## What was built

* [`adapters/python/octoprint/`](../../../adapters/python/octoprint): a read-only out-of-process
  adapter (standard library only), its manifest and descriptor, a test harness (a real
  OctoPrint plus a recording, fault-injecting proxy), 42 tests, recorded OctoPrint answers
  (`fixtures/`), a demo script, and pinned requirements for OctoPrint.
* A [registry entry](../../../registry/adapters/openmhs.py.octoprint.json) and a committed
  [conformance report](../../../conformance/reports/octoprint.virtual-01) (L1, `sources:
  ["simulator"]`), which the [compatibility matrix](../../compatibility.md) shows as
  integration-tested against a simulated peer.
* A CI job (`octoprint`) that runs the tests and the demo on Linux. It has not run yet: nothing
  was pushed.

No change was made to the OpenMHS core, its schemas, policy or tests.

## Reproduce it

You need Python 3.11 (OctoPrint 1.11.8 supports 3.7 to 3.13) and a built `openmhs`
(`cargo build -p openmhs-cli --bin openmhs`).

```sh
python -m venv .octoprint-venv
.octoprint-venv/bin/pip install -r adapters/python/octoprint/requirements.txt   # Windows: .octoprint-venv\Scripts\
export OPENMHS_OCTOPRINT=$PWD/.octoprint-venv/bin/octoprint
export OPENMHS_BIN=$PWD/target/debug/openmhs
```

PowerShell: `$env:OPENMHS_OCTOPRINT = "$PWD\.octoprint-venv\Scripts\octoprint.exe"`, and likewise
`OPENMHS_BIN`. If `python` is not on the gateway's `PATH`, set `OPENMHS_PYTHON`.

**The demo** starts a throwaway OctoPrint (random port and API key, scratch directory, Virtual
Printer enabled, OctoPrint's telemetry and update plugins disabled), a proxy and a gateway,
runs eleven steps with the real CLI, and removes everything:

```sh
python adapters/python/octoprint/demo.py --out transcript.txt --wire-log wire-log.json
```

**The tests:**

```sh
python -m unittest discover -s adapters/python/octoprint -v
```

Without `OPENMHS_OCTOPRINT` only the offline tests run (the adapter against a fake that replays
recorded OctoPrint answers). Without `OPENMHS_BIN` the gateway tests are skipped.

**Conformance** (the report in `conformance/reports/octoprint.virtual-01/` came from this):

```sh
python adapters/python/octoprint/run_conformance.py --report-dir out/
```

**To run the adapter by hand** against your own OctoPrint
(still the Virtual Printer, please), see the [adapter reference](../../adapters/octoprint.md#configuration).

## Using a real OctoPrint by hand

1. `pip install -r requirements.txt` as above, then `octoprint serve --basedir <empty dir>`.
2. Complete OctoPrint's first-run wizard in a browser at `http://127.0.0.1:5000`; enable the
   *Virtual Printer* plugin (it ships with OctoPrint) under *Settings → Virtual Printer*, and
   connect to port `VIRTUAL`.
3. Create an application key under *Settings → Application Keys* and export it as
   `OCTOPRINT_API_KEY`. Keep it out of the repository and out of logs.
4. `openmhs serve --adapter-manifest adapters/python/octoprint/manifest.json --allow-unsigned-adapters`

## Known limitations

See the [report](REPORT.md#known-limitations).

## Recording plan (GitHub)

A 60 to 90 second terminal recording, no voice needed. Record `demo.py` itself, so every frame
is real output:

1. Title card or first line: "OpenMHS, Experiment 01: a virtual 3D printer. No hardware."
2. Run `python adapters/python/octoprint/demo.py`. It pauses on nothing; use `vhs` or
   `asciinema rec -c "python adapters/python/octoprint/demo.py"` and trim nothing.
3. Hold on step 6 (write granted, still refused, "requests that reached OctoPrint: 0"), step 7
   (unplugged: `adapter.disconnected`, `printer.state = offline`), and step 11 ("read-only on the
   wire: yes").
4. End on the last two lines (*Simulated: ... Not verified: ...*). Do not crop them.

Do not call the result "safe", "certified" or "tested on a printer". Caption: "Simulated peer:
OctoPrint's Virtual Printer plugin. Physical printers not tested." Upload the `.cast` or GIF
to the repository's Discussions or the release page, not into the README, until the Linux CI
job has passed.
