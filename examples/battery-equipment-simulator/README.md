# A read-only agent on simulated battery test equipment

**Everything here is SIMULATED and the register map is EXAMPLE-ONLY.** No real instrument,
vendor, device or battery is involved, and no value in this example describes one.

```
simulated battery test instrument      OpenMHS Modbus adapter      gateway policy + credential      read-only agent
 (Modbus TCP on localhost)        ->   (EXAMPLE-ONLY profile)  ->  (reads allowed, writes denied) ->  (cited answers)
```

An agent asks the gateway for the instrument's status, a telemetry snapshot, or a completed test
session. It gets back values with units, timestamps, provenance and a quality state, and it cites
them. It cannot change a limit or start or stop a test: the credential has no scope for it, the
policy denies it, and the simulator itself refuses every write. The refusals are audited.

## Run it

Build the gateway once (`cargo build -p openmhs-cli`), or point `OPENMHS_BIN` at an `openmhs`
binary. Python 3.10 or later, standard library only. No hardware, no network beyond localhost,
no credentials.

```sh
python examples/battery-equipment-simulator/run_example.py
python -m unittest discover -s examples/battery-equipment-simulator -p "test_*.py" -v
```

`run_example.py` starts a throwaway gateway per scenario and prints each answer with its evidence.
The tests fail loudly, rather than skip, if the binary is missing.

### Try it by hand

```sh
# terminal 1: the simulated instrument (a fixture from fixtures/)
python examples/battery-equipment-simulator/simulator.py --fixture session

# terminal 2: a gateway that reaches it (the profile says `simulated: true`)
openmhs serve --profile examples/battery-equipment-simulator/battery-test-instrument.profile.json

# terminal 3: a fresh gateway denies everything until a grant exists
openmhs grants add --id battery-read --principal 'operator:*' --device sim.battery-test-01 \
  --capability '*' --ops read --effect allow
openmhs read sim.battery-test-01 telemetry.voltage     # 3.65 V (GOOD, simulator)
openmhs read sim.battery-test-01 session_1.id          # 1002
openmhs exec sim.battery-test-01 limit.voltage_max --value 5V   # DENIED: no grant
```

`openmhs serve --profile-sim ...` instead serves the profile's built-in simulator (a quick start
with a few seed values; it has no sessions or fixtures).

## What it shows

| Scenario | Fixture | What the agent does |
|---|---|---|
| Normal operation | `normal` | Reports status, state and a telemetry snapshot, each reading cited |
| Completed session with samples | `session` | Lists two sessions, retrieves one with its four samples |
| Stale data | `stale` | Shows the values, flags them STALE and says not to treat them as current |
| Suspect data | `suspect` | Shows the values flagged SUSPECT; does not correct or hide them |
| Malformed data | `malformed` | The adapter refuses an out-of-range register; the answer says "no value" |
| Missing, out-of-order, suspect samples | `irregular_session` | Missing stays missing; a sample that goes back in time is flagged, not reordered |
| Disconnect and reconnect | `normal` + `set_link` | Reads fail as UNREACHABLE while the link is down and recover after it |
| Write and control requests | `normal` | Denied for a read-only credential and, separately, by policy alone; audited; none reach the device |

### Data-quality states

OpenMHS qualities are `GOOD`, `UNCERTAIN` and `BAD`, plus freshness. The example's vocabulary is
built from what the gateway and the instrument each say, and it never invents a state:

| State | Where it comes from |
|---|---|
| `OK` | gateway quality `GOOD`, usable, in range, and the instrument's `telemetry.data_quality` is `ok` |
| `STALE` | the instrument's own `telemetry.sample_time` is more than 5 s older than the read (an example policy, `STALE_AFTER_S`), or the gateway marks the reading not usable |
| `SUSPECT` | `telemetry.data_quality = suspect`, a sample flagged `suspect`, or a gateway quality other than `GOOD` |
| `UNREACHABLE` | the read fails with `adapter.disconnected`, `adapter.timeout` or `device.unavailable` |
| `MALFORMED` | the adapter rejects the register (`adapter.malformed_response`, e.g. 65.535 V against a declared 0 to 60 V) |
| `MISSING` | the instrument flags the sample `missing`; no value is shown |
| `OUT_OF_ORDER` | a sample slot is timestamped earlier than the slot before it |

A Modbus reading is stamped when the adapter reads it, so the gateway's freshness limit guards
the link, not the instrument. That is why staleness is judged from the instrument's own
timestamp register.

## Capabilities

The profile (`battery-test-instrument.profile.json`) is generated from `register_map.py`, which is
also what the simulator serves, so the two cannot drift. `build_profile.py --check` fails if the
committed profile is out of date.

| Group | Capabilities | Agent permission |
|---|---|---|
| Identity and status | `instrument.simulated`, `instrument.state` | read |
| Telemetry | `telemetry.voltage`, `.current`, `.temperature`, `.sample_time`, `.data_quality` | read |
| Sessions (2 slots, newest first) | `session.count`, `session_N.id`, `.asset_id`, `.procedure_version`, `.status`, `.sample_count`, `.start_time`, `.end_time` | read |
| Samples (4 per session) | `session_N.sample_M.time`, `.voltage`, `.current`, `.temperature`, `.valid` | read |
| Control points | `limit.voltage_max` (write), `test.start`, `test.stop` (actions) | **none** |

The control points exist only so the permission layer has something real to deny: without them a
write request would fail as "no such capability", which proves nothing about permissions.

### How read-only is enforced, in four places

1. **Credential scope.** The agent token has scopes for the data capabilities, operation `read` only.
2. **Policy.** Grants allow `read` on those capabilities; a `deny` grant covers every `write` and
   `invoke`. An unscoped credential is still refused (`policy.denied.explicit`).
3. **Audit.** Denied commands and denied reads are recorded (`command.transition` to `DENIED`,
   `read.denied`) and the chain verifies.
4. **The simulator.** Every Modbus write function is answered with exception 01 and counted. The
   tests assert the count is zero after the agent's attempts.

## The simulator profile (EXAMPLE-ONLY)

Not a standard and not any vendor's map. Unit id 1, port 15020 by default; the runner picks a free
port. Tables: discrete input 0; input registers 0 to 7 (state, data quality, voltage 1 mV, current
10 mA, temperature 0.1 C, 32-bit sample time, session count), then session slot 1 at 10 to 43 and
slot 2 at 50 to 83; holding register 0 and coils 0 and 1 for the control points. The full list is
`register_map.py`. Identifiers are numeric codes because Modbus carries numbers; times are Unix
seconds. Fixtures are in `fixtures/`; each says what it is for and all dates are fixed.

The simulator is about 200 lines of standard-library Python (reads of function codes 1 to 4, every
other function refused). PyModbus is not used, so there is no dependency or licence review. A
battery model such as PyBaMM could generate fixture traces; it is not included and would stay
outside the OpenMHS dependencies.

## Files

| File | Purpose |
|---|---|
| `register_map.py` | The single source of truth for the register map |
| `build_profile.py`, `battery-test-instrument.profile.json` | Generate and hold the OpenMHS Modbus profile |
| `simulator.py` | The simulated instrument (Modbus TCP, read-only) |
| `fixtures/*.json` | Deterministic scenarios |
| `battery_lab.py` | Starts simulator, gateway, grants and credentials; opens an MCP session |
| `agent.py` | The scripted read-only agent, the data-quality states, and the grounding check |
| `run_example.py` | Runs every scenario and prints cited answers |
| `test_battery_example.py` | 28 tests |

The core change this example needed is one optional profile field, `connection.simulated`, so
readings from a simulator you run yourself are recorded as `source: simulator`
([modbus.md](../../docs/adapters/modbus.md)). Existing profiles behave as before.

## What the agent's answers are, and are not

The agent is scripted, with no model. Each statement cites the readings behind it. `check_grounding`
fails an answer if a number is not in a cited reading, a statement has no citation, or it uses
words that would turn a reading into a judgement (health, degradation, diagnosis, remaining life,
advice, prediction). That check is lexical: it catches invented numbers and obvious overreach, and
it cannot prove an answer is faithful in every other way.

## What this does and does not demonstrate

It demonstrates the governance path: discovery, scoped read access, provenance and quality on every
reading, honest handling of stale, suspect, malformed, missing, out-of-order and unreachable data,
and denied and audited writes.

It does **not**:

- show compatibility with any real battery equipment or any real Modbus instrument;
- validate a battery model, or say anything about the state of a battery;
- provide health scores, diagnoses, remaining-life estimates or advice;
- make OpenMHS a production gateway, or the simulator a model of a real instrument;
- test a real network, real timing, or real device faults;
- certify safety. Read-only enforcement here is software policy plus a simulator that refuses writes;
  a real instrument would need its own protection.

## Known limits

- **Modbus carries numbers.** Identifiers are numeric codes, and sessions are fixed slots (2 sessions
  of 4 samples), not an open-ended list or a time series. A real integration would need a richer
  protocol or a purpose-built adapter.
- **Gateway quality is always `GOOD` for Modbus reads.** `SUSPECT` here comes from flags the
  instrument's registers carry, which is a modelling choice of this profile.
- **Staleness uses a 5 s example threshold** and the instrument's own timestamp; a real instrument may
  not offer one.
- **Sample ordering** is judged on timestamps within one session only.
- **The disconnect scenario** drops the simulator's connections. It does not model partial replies,
  slow links or a device that answers wrongly while seeming healthy.
- **Tested on** Python 3.13 on Windows against a locally built gateway. CI runs the same tests with
  Python 3.10.
