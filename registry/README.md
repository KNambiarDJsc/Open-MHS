# Adapter registry

A git-based list of OpenMHS adapters. One JSON file per adapter in
[`adapters/`](adapters/), each pointing at the conformance report that supports it. The
[compatibility matrix](../docs/compatibility.md) is generated from these files and the
reports.

Listing is **not endorsement**, and a conformance level is **not a safety certification**.
Nobody reviews an adapter's code for safety before it is listed.

## Categories

| Category | Meaning | Needs |
|---|---|---|
| `official` | Maintained in this repository | A passing report, nothing failed or skipped |
| `community` | Maintained elsewhere | A passing report, nothing failed or skipped, and an `https` source |
| `experimental` | Partial, or evidence is incomplete | May have a report that is incomplete, or none |
| `unverified` | Listed without evidence | No report |

The matrix never shows an adapter as better than its report: no report means "Not
verified", skipped mandatory checks mean "Incomplete", and a report from a simulator is
never "Physically validated".

## Entry fields

Schema: [`schema/adapter-entry.schema.json`](schema/adapter-entry.schema.json).

| Field | Meaning |
|---|---|
| `id` | The adapter's own id (`adapter.id`). The file is `adapters/<id>.json` |
| `name`, `summary` | Human-readable |
| `protocol` | `builtin`, `modbus-tcp`, `mqtt`, `scpi`, `http`, `opc-ua`, `ros2`, `other` |
| `maintainer` | A name or GitHub handle. No email addresses |
| `source` | An `https` URL, or a path in this repository |
| `profiles` | Device profiles or manifests shipped with it |
| `version` | The adapter version the evidence is for |
| `license` | SPDX expression |
| `conformance_profile` | The level the adapter declares: `L0`–`L3`, or `null` |
| `category` | See above |
| `peer` | What the report tested against: `in-process-simulator`, `protocol-simulator` or `device`. `device` is refused unless the report's sources are not a simulator |
| `report` | `null`, or `{path, sha256, verified_at}`: a report under `conformance/reports/`, pinned by hash, and the date it ran |
| `notes` | Limits worth knowing |

## Adding an adapter

1. Build and test your adapter ([guide](../docs/adapters/README.md)). Run the suite:
   `openmhs conformance --profile <profile>` or `--adapter-manifest <manifest>`, with
   `--report-dir` to write the JSON report. Run it against a simulator or fixture, not a
   device that could be harmed.
2. Add the report as `conformance/reports/<device>/<date>-v<runtime>-<os>-<arch>.json`.
   Never edit or delete an existing report; a new run is a new file.
3. Add `registry/adapters/<id>.json` with the report's SHA-256
   (`sha256sum conformance/reports/...`) and the date.
4. Run `python tools/check_registry.py` and `python tools/gen_matrix.py`, and commit
   the regenerated [compatibility.md](../docs/compatibility.md) and README table.
5. Open a pull request using the template. Or open an issue with the "Adapter registry
   listing" form if you want help.

### What the checks do

[`tools/check_registry.py`](../tools/check_registry.py) reads JSON only. It never runs,
imports or downloads an adapter, so a malicious entry cannot execute anything in CI. It
rejects: unknown or malformed fields, a file name that is not the id, paths that leave the
repository or do not exist, a missing report, a report that does not match its hash, a
report for another adapter, version or declared level, a summary that does not match its
checks, an `official` or `community` entry whose report failed or skipped anything, a
`device` peer backed by simulator readings, and an `unverified` entry that has a report.

Reports are validated against the
[conformance report schema](../spec/schemas/conformance-report.schema.json). Tests:
`python -m unittest discover -s tools -p "test_*.py"`.

## Community adapters

Community entries live here as data; the adapter itself stays in your repository.
Hardware reports from real devices are the most valuable thing a contributor can add. If
you ran an adapter against physical hardware, say so in the pull request and attach the
report; a maintainer will review before any entry is labelled physically validated.
