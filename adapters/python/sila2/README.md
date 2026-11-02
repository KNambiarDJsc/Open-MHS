# SiLA 2 adapter (generic client)

An out-of-process OpenMHS adapter for **any** SiLA 2 server, through the SiLA 2 Python SDK
([sila_python](https://gitlab.com/SiLA2/sila_python), MIT). It reads the Feature Definitions
the server reports and builds the OpenMHS descriptor from them. No code changes per
instrument; one reviewed override file per instrument type.

## What comes from where

| From the Feature Definition | From the reviewed override file |
|---|---|
| Properties: identifiers, descriptions, types, ranges, sets, lengths | Which features and commands are exposed: anything not named is **not exposed** |
| Units, matched to OpenMHS's table by structure (SI base units, factor, offset), not by label | Side-effect class and idempotency (never inferred from a name) |
| Commands' parameters and their ranges | Narrower parameter ranges (an override may narrow a range, never widen it) |
| Server name, type, UUID, version (as device name, model, serial, firmware) | Preconditions, readback verification, timing |
| | Which defined execution errors are raised before any effect (so they mean "nothing applied") |

Elements that cannot be mapped are left out and listed, with the reason, in the mapping report
(`config.report_file`). This version leaves out:
- a Real without a unit;
- units with no OpenMHS equivalent;
- lists, structures and data-type identifiers;
- observable commands.

Observable properties are read-only (no subscription).

## Errors

| SiLA | OpenMHS |
|---|---|
| `ValidationError` (the server refused the parameters) | `REJECTED`: nothing was applied |
| `DefinedExecutionError` listed in `defined_errors_before_effect` | `REJECTED`, with its fully qualified identifier and message |
| Any other defined or undefined execution error | Device failure (`FAILED`) |
| No answer within the override's `timeout_ms` | `UNKNOWN_OUTCOME`: the server may still be executing it |
| Connection lost | Disconnected (outcome uncertain) |

## Run

```sh
pip install -r requirements.txt
openmhs serve --adapter-manifest adapters/python/sila2/manifest.json --allow-unsigned-adapters
```

`config.server` is `sila://host:port` and must also appear in `endpoints.network`. This
prototype connects without TLS (`insecure`), so use it only against local, simulated servers.
The SDK is imported at module level: grpcio is a native extension (see
`research/lab-automation/02_opentrons`, finding 1).

**`config.reading_source` says what the server is:** `device` (the default) or `simulator`. The
adapter cannot tell, so readings carry what you say. It also decides conformance: the suite really
sends commands, so `openmhs conformance` refuses this adapter unless `reading_source` is `simulator`.
Do not set that on a server connected to an instrument. Faults are injected in the adapter, between
it and the server. Against the simulated servers in
[research/lab-automation/04_sila2](../../../research/lab-automation/04_sila2/) it reaches L3; that
says nothing about any instrument.
