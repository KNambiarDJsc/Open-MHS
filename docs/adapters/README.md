# Writing an OpenMHS adapter

This guide takes you from nothing to a tested adapter for your device. You need to know
some Rust. You do not need to know anything about OpenMHS: everything the guide relies
on is explained here, with links to the specification for detail.

> **A conformance level is not a safety certification.** OpenMHS checks that your
> adapter tells the truth about the device; it does not make the device safe to operate.
> Develop and test against the fake device. Connect real hardware only with the
> operator's approval and with the device's own safety systems (interlocks, limits,
> emergency stop) in place.

OpenMHS is an independent open-source project, not affiliated with Anthropic.

## 1. What an adapter is

OpenMHS is a gateway between AI agents (and people) and programmable hardware. Every
read and command goes through it:

```text
agent / CLI / SDK  ->  gateway  ->  adapter  ->  link  ->  device
                       (checks,     (your code)  (TCP,
                        journal,                  serial,
                        verification)             ...)
```

The **gateway** does the hard, generic parts before your code runs. It checks that the
caller is allowed, that values have the right type, unit and range, that rate and slew
limits and preconditions hold, and it writes every command to a journal before it is
sent. After your code reports back, it verifies the result by reading the device back.

The **adapter** is the part that knows your device. It does three things:

1. **Describe** the device: a JSON *descriptor* lists every capability with its type,
   unit, range, timing, side effects and how to verify it. Nothing undeclared can be used.
2. **Read** properties, saying when each value was taken, where it came from and whether
   it can be trusted.
3. **Execute** writes and actions: send each command once, and report exactly what the
   device said, including whether it might have acted when something went wrong.

The **conformance suite** checks all of this automatically, against a fake device that
can misbehave on demand.

## 2. Before you start

- Rust 1.89 or newer (`rustup update`).
- The `openmhs` command line. Until releases are published, build it from a checkout:

  ```sh
  git clone https://github.com/KNambiarDJsc/Open-MHS openmhs
  cd openmhs
  cargo build --release -p openmhs-cli      # the binary is target/release/openmhs
  ```

  Put `target/release` on your `PATH`, or call the binary by its full path.

## 3. Create your adapter

```sh
openmhs adapter new bench-lamp --openmhs-path /path/to/openmhs
```

This writes a crate called `bench-lamp` into `./bench-lamp`, built on the OpenMHS crates
in that checkout. Pass `--path <dir>` to put it somewhere else. Without `--openmhs-path`
the crate takes OpenMHS from the repository's `main` branch over Git instead.

The name is used for the crate, the adapter id (`bench-lamp`) and the device id
(`bench-lamp-01`). It must start with a lowercase letter and use only lowercase letters,
digits and single hyphens. The command refuses to write into a directory that is not
empty.

The new crate is a complete, working adapter for an example dimmable lamp that speaks a
simple line protocol, with a fake lamp to test against. You will turn it into your
device step by step. Its files:

| File | What it is |
|---|---|
| `descriptor.json` | What the device is and can do |
| `src/lib.rs` | The adapter: reads, writes, actions, and how each failure is classified |
| `src/link.rs` | The connection to the device: one request line out, one reply line back, over TCP |
| `src/fixture.rs` | A fake lamp speaking the same protocol, with faults on demand |
| `tests/conformance.rs` | Runs the conformance suite against the fake lamp |
| `examples/serve.rs` | Serves the adapter in a local gateway |

## 4. Run the conformance suite

```sh
cd bench-lamp
cargo test
```

The first build compiles OpenMHS and its dependencies and takes a few minutes. Later
builds take seconds. You should see:

```text
test reaches_its_declared_level ... ok
test reaches_its_declared_level_over_tcp ... ok
```

The suite runs twice: in memory on simulated time (fast and deterministic), and over a
real TCP socket, so the link's own error handling is tested too. Each run writes a full
JSON report to `target/tmp/conformance/` (`in-memory.json`, `tcp.json`).

### Levels

The descriptor claims a level in `adapter.conformance`; the suite measures the level the
adapter actually reaches. The test fails when the reached level is lower than the claim.

| Level | Name | Means |
|---|---|---|
| L0 | Discoverable | Valid descriptor; connects, reports health, shuts down cleanly |
| L1 | Observable | Every readable property reads correctly, and a lost link shows as lost, never as fresh data |
| L2 | Controllable | Every write and action is carried out, verified as declared, and every failure is classified honestly |
| L3 | Coordinated | Multi-device plans with resource leases. Not defined yet: no adapter can reach it |

A read-only sensor declares L1. Anything that can be commanded should reach L2.

### Reading a failure

When a test fails, it prints a summary. This is real output: the adapter was taught a
new capability, but the fake lamp was not (section 6 shows the whole change):

```text
bench-lamp-01 (bench-lamp 0.1.0): reached L0, declared L2 -> FAIL
  fixture faults: link_down, no_response, lost_ack, garbled, refuse; counts deliveries: yes
  49 pass, 2 fail, 10 untested, 3 not applicable, in 3000 ms
  L0 Discoverable    7 pass   0 fail   0 untested   0 n/a
  L1 Observable     22 pass   1 fail   3 untested   1 n/a
  L2 Controllable   20 pass   1 fail   6 untested   2 n/a
  L3 Coordinated     0 pass   0 fail   1 untested   0 n/a
  keeping it from L2:
    FAIL L1.read [lamp.color_temp]: read failed: [adapter.malformed_response] unexpected reply "ERR unknown query"
    UNTESTED L1.read.value [lamp.color_temp]: needs a reading; see L1.read
    ...
    FAIL L2.command [lamp.color_temp]: a valid command ended REJECTED (adapter.rejected: unknown command); the device refused a command the descriptor says is valid
    UNTESTED L2.verification [lamp.color_temp]: the command was not carried out; see L2.command
    UNTESTED L2.reject [lamp.color_temp]: needs a command that is carried out; see L2.command
    ...
```

Each line is one check: its status, the requirement id, the capability in brackets, and
what happened. Fix the `FAIL` lines first. An `UNTESTED` line names the check it was
waiting for, and passes or fails on its own once that one works. Every requirement id is
explained in [spec/conformance.md](../../spec/conformance.md#4-requirements).

## 5. The rules your adapter must follow

These rules are what the suite tests. The template already follows them; keep them as
you change it.

### Readings

- Report values in **exactly the unit the descriptor declares**. If the device answers
  in millivolts and the descriptor says `V`, convert in the adapter.
- Stamp each reading with **when it was measured** (`timestamp`), **where it came from**
  (`source`: `Device` for real hardware, `Simulator` for a fake), and its **quality**.
- When the link is down, **never return an old value as `GOOD`**. Fail with
  `Disconnected` (or `Timeout`), or return the value marked `BAD`. Agents make decisions
  on readings; a stale value that looks fresh is the most dangerous bug an adapter can
  have.

### Commands

- **Send each command once. Never retry.** If anything goes wrong, return the error; the
  gateway decides what happens next and tells the caller the truth. A retried
  "dispense 5 ml" dispenses 10.
- Call `progress.accepted()` only when the device has acknowledged the command.
- Report **only what the device said**. Do not report success because nothing went
  wrong that you noticed.
- **Classify every failure by whether the device might have acted.** This decides
  whether the gateway can say "nothing happened" or must say "unknown, reconcile before
  acting again":

| What happened | Return | The command ends |
|---|---|---|
| The device said it did it | `Ok(ExecuteReport::Applied { .. })` | `APPLIED`, then `VERIFIED` by readback |
| The device refused, and did nothing | `Ok(ExecuteReport::Rejected { reason })` | `REJECTED` |
| Nothing was sent (no connection, bad parameter) | `AdapterErrorKind::NotSent` | `FAILED`: certainly did not happen |
| The device reported it tried and failed | `AdapterErrorKind::DeviceFailure` | `FAILED` |
| Sent, but no answer in time | `AdapterErrorKind::Timeout` | `UNKNOWN_OUTCOME` |
| Sent, then the connection broke | `AdapterErrorKind::Disconnected` | `UNKNOWN_OUTCOME` |
| The device answered something unreadable | `AdapterErrorKind::MalformedResponse` | `UNKNOWN_OUTCOME` |

When in doubt, choose the uncertain answer. Wrongly saying "nothing happened" is a
safety problem; wrongly saying "unknown" costs a readback.

The template's `link.rs` reports link failures in three classes so that `lib.rs` can map
them: `Down` (nothing was sent), `Lost` (the connection broke after sending) and
`Timeout`. If you replace the link, keep the three classes.

## 6. Worked example: add a capability

The steps below were followed, exactly as written, in a clean directory
([recorded run](walkthrough.md)). They add a colour-temperature setting to the lamp. To
add one to your own device, do the same three things: declare it, implement it in the
adapter, and teach the fake device.

**1. Declare it** in `descriptor.json`, as a new entry in `capabilities`:

```json
{
  "name": "lamp.color_temp",
  "kind": "property",
  "description": "Colour temperature of the light.",
  "operations": ["read", "write"],
  "value": { "type": "number", "quantity": "temperature", "unit": "K", "range": { "min": 2700, "max": 6500 } },
  "side_effect": "reversible",
  "idempotent": true,
  "timing": { "expected_ms": 50, "timeout_ms": 1000 },
  "verification": { "mode": "readback", "tolerance": { "abs": 10 }, "timeout_ms": 1000 }
},
```

**2. Implement it** in `src/lib.rs`. The device protocol is `CT?` to read and `CT <kelvin>`
to set. Add one arm to `read`:

```rust
"lamp.color_temp" => quantity(number(&self.query("CT?").await?)?, "K"),
```

and one to the `match` in `execute`:

```rust
("lamp.color_temp", Some(Value::Quantity(q))) => format!("CT {}", q.value),
```

The gateway has already checked that the value is a quantity in kelvin and within
2700..6500 K, so the adapter does not repeat those checks.

**3. Run `cargo test`.** It fails, with the output shown in section 4: the fake lamp
answers `ERR unknown query` because it does not know `CT` yet.

**4. Teach the fake lamp** in `src/fixture.rs`. Add the state to `struct Lamp` and give
it a starting value in `FakeLamp::new`:

```rust
color_temp: f64,                 // in struct Lamp
color_temp: 4000.0,              // in FakeLamp::new
```

answer the query in `respond`:

```rust
"CT?" => format!("{:.0}", lamp.color_temp),
```

and apply the command in `apply`, refusing what the real device would refuse:

```rust
("CT", Some(a)) => match a.parse::<f64>() {
    Ok(k) if (2700.0..=6500.0).contains(&k) => {
        lamp.color_temp = k;
        Ok(())
    }
    _ => Err("colour temperature must be 2700..6500 K".into()),
},
```

**5. Run `cargo test` again.** Both tests pass. In `target/tmp/conformance/tcp.json`,
the new capability has passed every check, including the faults: refused (`REJECTED`),
garbled and unanswered (`UNKNOWN_OUTCOME`), and an applied command whose answer was lost,
which the gateway reconciled as applied by reading the value back.

You did not write any fault handling for the new capability. The template's error
mapping already covers it, and the suite proved it.

## 7. Make it your device

Work through these in order, running `cargo test` after each step.

1. **Describe your device.** Edit `descriptor.json`: the `device` block, then one
   capability per thing an agent may read, set or do. The field reference is
   [spec/descriptor.md](../../spec/descriptor.md); worked descriptors for a power supply,
   a linear axis and a temperature sensor are in
   [spec/examples/valid/](../../spec/examples/valid/). Fields that matter most:
   - `side_effect`: `none`, `reversible`, `irreversible` or `hazardous`. Grants can
     limit agents by it, so be honest. Anything that moves, heats, energizes or
     dispenses is at least `irreversible`.
   - `idempotent`: `true` only if sending the command twice has the same effect as once
     ("set to 5 V" is; "move 10 mm further" and "flash twice" are not).
   - `verification`: `readback` names the property that shows the command took effect.
     Use `mode: none` with a `reason` only when nothing can be read back. Such commands
     end `UNVERIFIABLE`, never `VERIFIED`.
   - `preconditions`: rules the gateway checks before sending, such as "the lamp must be on
     to flash" or "the axis must be homed to move".
   - `rate_limit` and `slew`: how often commands may be sent, and how fast a numeric
     setpoint may change. The gateway enforces both.
   - `timing`: how long a command usually takes, and when to give up.
2. **Speak your protocol.** Change the requests and replies in `src/lib.rs`, and the
   matching answers in `src/fixture.rs`. If the device does not use a line protocol over
   TCP, replace `TcpLink` in `src/link.rs` with what it needs (serial, Modbus, HTTP, a
   vendor library) and keep the `Down` / `Lost` / `Timeout` classes.
3. **Keep the fake faithful.** The fake device is only useful if it behaves like the
   real one: the same replies, the same refusals, the same state changes. Every fault in
   the suite is produced by the fake, so leave its fault handling as it is:

   | Fault | What the fake does |
   |---|---|
   | `link_down` | Every request fails, and over TCP the connection drops |
   | `no_response` | State-changing requests are not applied and get no answer |
   | `lost_ack` | They are applied, and the answer never arrives |
   | `garbled` | They are applied, and the answer is unreadable |
   | `refuse` | They are refused (`ERR ...`) and not applied |

   The fake also counts deliveries: state-changing requests that reached it. That is how
   the suite catches an adapter that retries.
4. **Rename things.** `LampAdapter`, `FakeLamp` and the `lamp.` capability names are
   yours to change. `ADAPTER_ID` in `src/lib.rs` must stay equal to `adapter.id` in the
   descriptor.
5. **Declare your level** in `adapter.conformance`: `L2` for anything commandable, `L1`
   for a read-only device.

## 8. Serve it in a gateway

`examples/serve.rs` starts a local gateway with your adapter connected to the fake
device:

```sh
cargo run --example serve -- --home ./home
```

In another terminal, use the `openmhs` command line against the same home directory.
The gateway denies everything until the operator grants it, including to the operator.

```sh
openmhs --home ./home devices list
openmhs --home ./home grants add --id me-lamp --principal operator:local \
  --device bench-lamp-01 --capability '*' --ops read,write,invoke --effect allow
openmhs --home ./home read bench-lamp-01 lamp.color_temp
openmhs --home ./home exec bench-lamp-01 lamp.color_temp --value 3000K
```

The last command prints the receipt: `VERIFIED`, with the readback that confirmed it.
Try a value out of range, or `lamp.flash --param count=2` while the lamp is off, and
the gateway refuses it before anything reaches the adapter. The full command line is in
[docs/cli.md](../cli.md).

To embed the adapter in your own program, push it into `GatewayConfig::adapters` before
`openmhs_gateway::start`, as `serve.rs` does.

### Real hardware

To drive a real device, replace the fake lamp in `serve.rs` with a link to it:

```rust
let link = Arc::new(TcpLink::new("192.168.1.50:5025"));
let adapter = LampAdapter::new(link, Arc::new(SystemClock));
```

Readings then carry source `device`. Before you do this:

- get the operator's approval to connect, every time;
- make sure the device's own safety systems are in place and work. OpenMHS is not one;
- start with grants that allow reads only, then add writes one capability at a time,
  with `--effect require_approval` for anything `irreversible` or `hazardous`.

Do not point the conformance suite at real hardware. It really sends commands, including
every action and hazardous capability, with values chosen from the descriptor's ranges.

## 9. When something fails

| Failure | Usual cause |
|---|---|
| `L0.descriptor` | The descriptor is invalid; the message names the field. Check it against [spec/descriptor.md](../../spec/descriptor.md) |
| `L0.adapter_id` | `ADAPTER_ID` differs from `adapter.id` in the descriptor |
| `L1.read` / `L2.command` on one capability | The adapter or the fake does not handle that request: check both sides of the protocol |
| `L1.read.value`: "readings must use the declared unit" | Convert to the declared unit in `read` |
| `L1.disconnect.read`: "returned a GOOD reading ... while the link was down" | A cache served an old value. Fail with `Disconnected`, or mark it `BAD` |
| `L1.reconnect` | The link does not reconnect after an error. `TcpLink` reconnects on the next request; keep that |
| `L2.verification` | The readback did not match. Check the readback property and `tolerance`, and that the fake really applies the change |
| `L2.reject`: "a refusal is returned as ExecuteReport::Rejected" | A device `ERR` became an error. Return `Rejected` |
| `L2.malformed` / `L2.timeout`: "the adapter reported success" | A failure was swallowed. Return the error |
| `L2.no_retry`: "reached the device N times" | The adapter or the link re-sends after a timeout. Remove the retry |
| Untested checks about preconditions | The suite could not satisfy a precondition. Declare a writable property, or an action whose readback sets it (like a "home" action) |

## 10. What is not covered yet

- **Rust only.** Adapters in other languages, running out of process, come in M7.
- **Subscriptions.** The adapter contract has no push updates yet, so `L1.subscribe` is
  reported as not applicable.
- **L3.** Multi-device coordination is defined in M9; until then `L3.leases` is always
  untested.

Questions and fixes to this guide are welcome as issues on the repository.
