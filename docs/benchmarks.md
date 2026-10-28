# Benchmarks

`openmhs bench` starts a throwaway gateway with the virtual lab in the same process and
times real client calls over loopback HTTP, as any client makes them. It measures:

- audited reads, one at a time and from several clients at once;
- a verified write from submit to `VERIFIED`;
- how long an observation takes to reach a waiting `events.poll`;
- how evenly a watch samples.

```sh
openmhs bench                      # defaults: 500 reads, 20 writes, 8 concurrent clients
openmhs --json bench > bench.json  # machine-readable, with the build and machine it ran on
```

**What these numbers are not.** They are one machine, simulated devices and loopback HTTP.
They say nothing about a real instrument's latency (a device's own response time dominates
there) or about fleet capacity. Virtual-time tests elsewhere in the repository show
scheduling, not speed.

## Measured

2026-10-28, branch `dx/mcp-sim`, **debug build** (`cargo build`), Windows 11, x86_64, 16
logical CPUs. A release-build run follows.

| Measurement | p50 | p95 | p99 | max | n |
|---|---|---|---|---|---|
| `state.read`, one at a time | 1.60 ms | 1.87 ms | 1.98 ms | 2.41 ms | 500 |
| `state.read`, 8 clients at once | 9.21 ms | 11.69 ms | 15.63 ms | 16.83 ms | 496 |
| Write to `VERIFIED` (includes the descriptor's 50 ms settle) | 102.26 ms | 105.55 ms | 115.73 ms | 115.73 ms | 20 |
| Observation to a waiting `events.poll` | 0.25 ms | 0.29 ms | 0.35 ms | 0.35 ms | 50 |
| Gap between watch samples (200 ms watch) | 195 ms | 210 ms | 213 ms | 213 ms | 14 |

### Release build

Same day, same machine and branch, `cargo build --release` (built to a separate target
directory):

| Measurement | p50 | p95 | p99 | max | n |
|---|---|---|---|---|---|
| `state.read`, one at a time | 1.13 ms | 1.36 ms | 1.39 ms | 1.45 ms | 500 |
| `state.read`, 8 clients at once | 8.00 ms | 16.36 ms | 16.95 ms | 17.18 ms | 496 |
| Write to `VERIFIED` | 85.64 ms | 105.70 ms | 183.47 ms | 183.47 ms | 20 |
| Observation to a waiting `events.poll` | 0.03 ms | 0.35 ms | 0.46 ms | 0.46 ms | 50 |
| Gap between watch samples (200 ms watch) | 196 ms | 210 ms | 210 ms | 210 ms | 14 |

Concurrent reads reached about 851 per second, against about 843 in the debug build: an
optimized build barely moves throughput. That points to the journal's per-read audit write,
not computation, as the limit (consistent with the debug numbers below, not yet profiled).
Single-read latency fell by about a third and event delivery to a few tens of microseconds.

**Throughput.** Concurrent reads reached about 843 per second against about 625 one at a
time. Every allowed read writes an audit record, and the journal serializes those writes, so
more clients mostly add queueing. This is the growth the storage RFC addresses
(`TELEMETRY` class), and the reason watch samples are not audited one by one (ADR-0023).

**Writes** take about twice the declared 50 ms settle. The rest is two live-check reads
(precondition and slew), the journal transitions, and the readback poll.

**Watches.** The first run of this benchmark found watch samples drifting to about 211 ms on
a 200 ms watch: the loop slept the interval, then read. Watches now tick at a fixed rate and
skip a late tick rather than bunching samples. The run above delivered 15 of 15 expected
samples in 3 s.

## Not measured yet

- Linux and macOS.
- A real device over a real network.
- Startup and sweep time with N offline devices, in wall-clock time (the tests measure
  them in virtual time).
- Memory.
- More than one gateway.
