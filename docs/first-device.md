# First simulated device

You will give an agent a narrow permission on the simulated power supply, watch it be
refused, then succeed, and verify the audit log. Start a gateway first
(`openmhs serve --sim`, see the [five-minute tutorial](tutorial.md)); the commands below run
in a second terminal.

## 1. The power supply

Nothing is allowed until the operator allows it. Try to read the supply as the operator:

```text
$ openmhs read sim.psu-01 power.output.voltage.setpoint
error [policy.denied.no_grant]: no grant allows 'operator:local' to read 'power.output.voltage.setpoint' on 'sim.psu-01'
```

Allow an agent to read and write the supply, and issue it a credential. The token is
printed once.

```sh
openmhs grants add --id agent-psu --principal agent:demo --device sim.psu-01 --capability '*' --ops read,write --effect allow
openmhs tokens create agent:demo --any-scope
```

Act as the agent by setting the token (`export OPENMHS_TOKEN=omhs_...` in a Unix shell,
`$env:OPENMHS_TOKEN = "omhs_..."` in PowerShell), then ask for 5 V:

```text
$ openmhs exec sim.psu-01 power.output.voltage.setpoint --value 5V
state    DENIED  -> denied (nothing sent to the device)
policy   deny  (rules: agent-psu)
  denied command.slew_exceeded: change of 5 V exceeds the limit of 2 V per 1000 ms
```

The supply declares a slew limit, so the gateway refuses the jump and sends nothing to
the device. Ask for a smaller step:

```text
$ openmhs exec sim.psu-01 power.output.voltage.setpoint --value 2V
state    VERIFIED  -> succeeded (confirmed by readback)
verify   readback power.output.voltage.setpoint  expected 2 V  observed 2 V  (GOOD, simulator)  MATCH
```

`VERIFIED` means the gateway read the value back from the device and it matched. Other
mistakes are refused too, each with a stable code:

| Request | Result |
|---|---|
| `--value 48V` | `command.value.out_of_range`: 48 V is outside the allowed range 0..12 V |
| `--value 5A` | `command.value.unit_mismatch`: an ampere is not a volt |
| a device or capability you were not granted | `policy.denied.no_grant` |

An agent cannot grant itself access: `openmhs grants add ...` with the agent's token
fails with `gateway.forbidden: changing grants requires an operator token`.

## 2. Check the audit log

Unset the token to act as the operator again, then:

```text
$ openmhs audit verify
audit log intact: 21 records, hash chain verified
```

Every grant, read, command and transition is recorded, and the records are hash-chained.
`openmhs audit tail` shows the latest ones.

## Where next

- [Connecting an AI agent](mcp.md) puts a model in the agent's seat
- [Connect your own device](connect-a-device.md) does the same with your hardware
- [Concepts](concepts.md) explains the words used here
- [Simulators and fault injection](simulators.md) shows how to make the supply misbehave
