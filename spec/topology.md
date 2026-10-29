# Capability graph and topology (v0.1)

Schema: [`schemas/topology.schema.json`](schemas/topology.schema.json)
(`urn:openmhs:schema:0.1:topology`). Reference implementation:
`openmhs_types::graph`. Decision record: [ADR-0018](../docs/adr/0018-orchestration.md).

The gateway keeps a graph of what is registered: devices, their components, and how they
relate to each other, to shared resources and to coordinate frames. `graph.get` returns
it (`openmhs graph`).

## Sources

Relationships come from two places and use the same objects
([descriptor.md](descriptor.md) §7):

- **Descriptors**, written by adapter and profile authors: what is true of the device
  wherever it is installed (its parts, the frame its positions are in, what it physically
  contains).
- **The topology file**, written by the operator: what is true of this installation (which
  supply powers which stage, which devices share a bench). Given to the gateway with
  `openmhs serve --topology FILE`.

```json
{
  "openmhs_topology": "0.1",
  "relationships": [
    { "from": "sim.axis-01", "kind": "depends_on", "target": "sim.psu-01" },
    { "from": "sim.axis-01", "kind": "shares_resource", "resource": "workcell.bench" },
    { "from": "sim.thermo-01", "kind": "shares_resource", "resource": "workcell.bench" }
  ]
}
```

## Checks

- A topology is applied after every device is registered. Any relationship that starts
  from or targets an unregistered device (`topology.device.unknown`,
  `topology.target.unknown`), or breaks a descriptor relationship rule, **stops the
  gateway from starting**. A mistaken topology must not quietly leave devices unclaimed.
- A descriptor relationship to a device that is not registered is reported in the graph's
  `issues` (`graph.target.unregistered`) and otherwise ignored: devices register
  independently.
- A `contains` cycle is reported (`graph.contains.cycle`).
- Runtime topology changes are refused with `topology.busy` while a command holds its
  resource lock set; apply configuration before starting work.

## What the graph decides: claims

The graph is descriptive except when a lease is taken. Leasing a set of devices and
resources **claims**:

1. those devices and resources;
2. every device a claimed device `contains`;
3. every resource a claimed device `shares_resource`s;
4. every device that shares a claimed resource;

repeated until nothing more is added. Two leases conflict when their claims overlap.

`depends_on` does not claim its target: a stage that needs a supply does not stop others
from using the supply. A plan that needs both leases both. `frame` claims nothing.

The rule is deliberately conservative: two devices that share a workspace cannot be
operated by different holders at the same time, even if their motions might never meet.
OpenMHS does not reason about geometry.

Within one gateway process, state-changing commands also hold locks for every declared
shared resource used by their target device, from live checks through verification. A
command involving several shared resources takes their locks in sorted resource-id order.
This prevents two local adapters from concurrently changing a declared shared resource,
even when their device ids differ. The stop lane remains concurrent by design. These locks
do not fence another gateway process or a direct device controller; resource ownership
epochs and controller-enforced fencing are not implemented yet.

## Leases

A lease gives its holder exclusive use of what it claims until a deadline.

- **Acquire** (`leases.acquire`): devices and/or resources and a `ttl_ms` (1 ms to 24 h,
  required: there is no lease without an end). All or nothing, in one journal
  transaction: if anything the claim covers is already claimed by a live lease, nothing is
  taken (`lease.conflict`, naming what is shared, the other holder and its deadline).
- **Effect**: while the lease is live, every write and invoke on a claimed device by
  anyone but the holder is refused with `lease.held`. Reads are not affected. The check
  runs for every command, including one held for approval, which is checked again when
  approved. If the leases cannot be read, nothing runs (`gateway.journal`). Commands
  already running when a lease is taken finish; later writes sharing a resource serialize
  behind the current local command.
- **Who can lease**: only a caller that could operate every claimed device (its
  credential's scopes reach the device and some grant short of `deny` allows a write or
  invoke on it). A read-only agent cannot lock others out. A resource can be leased only
  by a caller that could operate every device sharing it.
- **Renew** (`leases.renew`): the holder moves the deadline to `ttl_ms` from now.
- **Release** (`leases.release`): the holder, or a human approver freeing devices a holder
  left leased.
- **End**: a lease ends when released or at its deadline; no sweeper is needed for the
  deadline to take effect. Endings past a deadline are recorded (`lease.expired`) the next
  time leases are read or the gateway recovers. Revoking or rotating a credential releases
  the leases it took.
- **Durable**: leases are journal records; a restarted gateway honours them and their
  deadlines.
- **Visible**: every caller can list and read every lease. Who holds a device is how
  callers coordinate.

The holder is the principal; the credential used is recorded. Stops are never blocked by
a lease (ADR-0018).
