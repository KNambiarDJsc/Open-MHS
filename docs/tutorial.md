# Five-minute tutorial

You will run a gateway with simulated devices, give an agent a narrow permission, watch
it be refused and then succeed, and verify the audit log. Nothing here touches hardware.
You need an `openmhs` binary ([installation](installation.md)).

## 1. Watch the guided tour (10 seconds)

```sh
openmhs demo
```

It starts a throwaway gateway and walks through denial by default, range, unit and slew
limits, human approval, a lost acknowledgement becoming `UNKNOWN_OUTCOME`, and audit
verification.

## 2. Start a gateway

In terminal 1:

```sh
openmhs serve --sim
```

```text
OpenMHS gateway listening on http://127.0.0.1:7400
  home     <your gateway home directory>
  device   sim.axis-01 (openmhs.sim.axis)
  device   sim.psu-01 (openmhs.sim.psu)
  device   sim.thermo-01 (openmhs.sim.thermo)
Press Ctrl+C to stop.
```

In terminal 2, list the devices and inspect one. Its descriptor is the contract: units,
ranges, preconditions, and how each write is verified.

```sh
openmhs devices list
openmhs devices inspect sim.psu-01
```

## 3. Next: your first simulated device

Continue with [First simulated device](first-device.md): give an agent a narrow
permission, watch it be refused, then succeed, and verify the audit log.

## 4. Connect an AI agent

The same gateway serves agents over MCP. A scripted agent needs no model and no key. Run
it from a checkout of the repository, after `cargo build -p openmhs-cli`:

```sh
python examples/agent-loop/mock_agent.py
```

To use Claude Code or another MCP host, see [Connecting an AI agent](mcp.md).

## Where next

- [Concepts](concepts.md) for the vocabulary
- [Simulators and fault injection](simulators.md) to break things on purpose
- [Writing an adapter](adapters/README.md) to connect a new device
- [The workcell plan](../examples/workcell/README.md) for multi-device coordination
