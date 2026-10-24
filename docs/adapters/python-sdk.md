# Writing an adapter in Python

`python/openmhs/adapter.py` lets you write an adapter in Python, for devices whose best
library is a Python one. The adapter runs as its own process; the gateway starts it from
a [permission manifest](../../spec/permission-manifest.md) and talks to it over stdin and
stdout ([protocol](../../spec/adapter-protocol.md)). It goes through the same conformance
suite as a Rust adapter. The SDK uses only the standard library and needs Python 3.10 or
newer.

> **A conformance level is not a safety certification.** Develop against a simulator.
> Connect real hardware only with the operator's approval and with the device's own
> safety systems in place.

Read [the adapter guide](README.md) first for what descriptors, readings and outcomes
mean. This page covers only what is different in Python.

## A complete adapter

`adapters/python/psu-sim/psu_sim.py` is a working example: the simulated power supply,
ported from Rust, reaching L2 with all five conformance faults. The shape is:

```python
from openmhs.adapter import Adapter, AdapterError, Applied, Rejected, quantity, reading, run

class Lamp(Adapter):
    adapter_id = "acme.lamp"          # = manifest adapter.id = descriptor adapter.id
    version = "0.1.0"

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        permissions.check_network(config["url"])   # refuse what the manifest did not declare
        self.url = config["url"]

    async def describe(self):
        return DESCRIPTOR                            # must not change between restarts

    async def read(self, capability):
        if capability != "lamp.on":
            raise AdapterError("unsupported", capability)
        on, measured_at = await self.query()
        return reading(on, timestamp=measured_at)    # quality "GOOD" by default

    async def execute(self, command, progress):
        try:
            await self.send(command.value)           # once; never retried
        except ConnectionRefusedError as e:
            raise AdapterError("not_sent", str(e))   # nothing reached the device
        except TimeoutError as e:
            raise AdapterError("timeout", str(e))    # it may have acted
        progress.accepted()
        return Applied()

if __name__ == "__main__":
    run(Lamp())
```

And its manifest:

```json
{
  "openmhs_manifest": "0.1",
  "adapter": { "id": "acme.lamp", "version": "0.1.0" },
  "runtime": {
    "command": ["python", "{manifest_dir}/lamp.py"],
    "env": { "PYTHONPATH": "/path/to/openmhs/python" }
  },
  "capabilities": { "allow": [{ "name": "lamp.*", "operations": ["read", "write"] }] },
  "endpoints": { "network": ["http://10.0.0.20:80"] },
  "config": { "url": "http://10.0.0.20:80" }
}
```

```sh
openmhs adapter check lamp/manifest.json
openmhs serve --adapter-manifest lamp/manifest.json
```

## Rules the SDK cannot enforce for you

- **stdout is the protocol.** `serve()` moves file descriptor 1 aside before your code
  runs, so `print()` and C libraries that write to stdout end up on stderr, which the
  gateway logs. Do not reopen `/dev/stdout`.
- **Send each command at most once.** Do not retry `execute`, not even after a timeout.
  The gateway records the outcome as unknown and a person or agent reconciles it.
- **Classify errors honestly.** `not_sent` only when you know nothing reached the
  device; `device_failure` when the device said it failed; `timeout`, `disconnected`,
  `malformed_response` when it may have acted. Return `Rejected(reason)` when the device
  refused and did nothing.
- **Timestamps are measurement times.** Pass the device's own timestamp to `reading()`
  when it has one. Never return an old value as `GOOD`: raise `disconnected`, or return
  quality `"BAD"`/`"UNCERTAIN"`.
- **No policy.** Do not second-guess whether a command is allowed, clamp values, or add
  your own approval step. The gateway has already decided; your job is to say what the
  device did.
- **Check permissions before opening anything.** `permissions.check_network(url)`,
  `check_serial(port)` and `check_read(path)` refuse what the manifest did not declare.
  They are cooperative checks, not a sandbox (see the manifest spec's enforcement table).
- **Secrets come from the environment.** List variable names under `credentials` in the
  manifest and read them with `os.environ`; the gateway redacts their values from your
  logs. Never put secrets in `config`.

## Conformance

Offer faults by setting `fixture_faults` (a subset of `link_down`, `no_response`,
`lost_ack`, `garbled`, `refuse`) and implementing `fixture_inject(fault)` and
`fixture_clear()`. If your simulator counts state-changing requests, set
`fixture_counts_deliveries = True` and call `self.report_deliveries(n)`. Fixture mode is
only switched on by `openmhs conformance`, and only works against a simulator:

```sh
openmhs conformance --adapter-manifest lamp/manifest.sim.json
```

An adapter that offers no faults gets no fixture, and the suite then sends it no test
commands.

## Testing failure handling

The host's own tests use `crates/openmhs-host/tests/adapters/misbehave.py`, an adapter
that crashes, hangs, writes garbage or changes its descriptor on request. Your adapter
does not need to handle any of that; the host contains it. See
[adapter-host.md](../architecture/adapter-host.md) for what happens in each case.
