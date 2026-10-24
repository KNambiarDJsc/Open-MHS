# OctoPrint adapter (read-only)

`adapters/python/octoprint/` reads a 3D printer's state and temperatures from an
[OctoPrint](https://octoprint.org/) server over its documented
[HTTP API](https://docs.octoprint.org/en/master/api/). It was written for
[Experiment 01](../experiments/01-octoprint/README.md): testing OpenMHS against a real
external service without owning any hardware, using OctoPrint's bundled **Virtual Printer**
plugin.

> **Read-only, and simulated.** The adapter cannot send G-code, start or cancel a job, set a
> temperature, move an axis or change OctoPrint's connection. Everything in this repository's
> evidence was gathered against the Virtual Printer. **No physical printer has been tested.**
> A conformance level is not a safety certification.

## What it exposes

| Capability | Value | Read from |
|---|---|---|
| `printer.state` | `offline`, `connecting`, `operational`, `printing`, `paused`, `error` | `GET /api/connection`, `current.state` |
| `hotend.temperature` | °C | `GET /api/printer`, `temperature.tool0.actual` |
| `hotend.target` | °C, the target OctoPrint last set (read-only here) | `temperature.tool0.target` |
| `bed.temperature` | °C | `temperature.bed.actual` |
| `bed.target` | °C, read-only here | `temperature.bed.target` |

A temperature's timestamp is the time of OctoPrint's last sample (`history[-1].time`), not the
time of the request. `source` is `simulator` when the printer port is `VIRTUAL`, otherwise
`device`; `printer.state` reads `offline` (quality `GOOD`) while the printer is unplugged,
because that is true and useful. Only `tool0` and the bed are mapped.

## How failures are classified

The adapter never retries, and it never writes, so no failure can leave a printer in an
unknown state: whatever happens on the link, the only requests it can have sent are `GET`s to
`/api/connection` and `/api/printer`. The tests check that on the wire.

| What happened | Error class |
|---|---|
| OctoPrint refuses or drops the connection, or the printer's serial link is closed | `adapter.disconnected` |
| OctoPrint does not answer within `timeout_ms` | `adapter.timeout` |
| `401`/`403` (bad API key), `404`, `5xx`, a redirect, temperatures still `null` after a connect | `adapter.device_failure` |
| The body is not JSON, is not an object, is over 1 MiB, or lacks the field | `adapter.malformed_response` |
| A capability that is not in the descriptor | `adapter.unsupported` |

`health` is `healthy` when the printer is operational, printing or paused;
`disconnected` when OctoPrint is unreachable or its printer connection is closed; and
`degraded` otherwise (for example a rejected key).

## Configuration

```json
{
  "endpoints": { "network": ["http://127.0.0.1:5000"] },
  "credentials": ["OCTOPRINT_API_KEY"],
  "config": { "url": "http://127.0.0.1:5000", "api_key_env": "OCTOPRINT_API_KEY", "timeout_ms": 2000, "descriptor": { "...": "see manifest.json" } }
}
```

* The API key is a **credential**: the manifest names an environment variable of the gateway,
  the gateway passes it and redacts it from the adapter's logs, and the adapter sends it only
  in the `X-Api-Key` header. It is not in the manifest, the descriptor, any result or any
  error message. Create a key in OctoPrint under *Settings → Application Keys*, or use the
  global key for a local trial.
* `url` must be declared in `endpoints.network`. Plain `http://` is accepted only for
  loopback, or with `allow_plaintext: true`; use `https://` for anything else, because the key
  travels in a header.
* The client ignores proxy environment variables and does not follow redirects, so the key goes
  only where `url` points.
* A descriptor that declares anything but `read` and `subscribe`, or a capability the adapter
  cannot read, is refused at start-up.

Run it behind a gateway:

```sh
export OCTOPRINT_API_KEY=...        # PowerShell: $env:OCTOPRINT_API_KEY = "..."
openmhs adapter check adapters/python/octoprint/manifest.json
openmhs serve --adapter-manifest adapters/python/octoprint/manifest.json --allow-unsigned-adapters
```

## Setting up a virtual printer

See the [experiment's README](../experiments/01-octoprint/README.md#reproduce-it): a virtual
environment with a pinned OctoPrint, the Virtual Printer plugin enabled, no hardware.
OctoPrint is AGPL-3.0; it is installed separately and nothing of it is bundled here.

## Limits

One printer per OctoPrint connection; only `tool0` and the bed; no job, file, webcam or
G-code access; no subscriptions beyond the gateway's polling; no authentication method but the
API key. The conformance report's `link_down` fault is injected inside the adapter (a real
printer disconnect is exercised by the integration tests instead).
