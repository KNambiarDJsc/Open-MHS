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

## Journal start-up

Before it serves anything, a gateway checks its journal: SQLite's `quick_check`, the full
`integrity_check` of the commands and approvals tables (their unique indexes stop duplicate
commands and double-spent approvals), and every link of the audit chain. Then recovery
settles the unfinished commands. The checks grow with the journal; since RFC 0002
milestone 1, recovery grows only with the unfinished commands ([PERSISTENCE_ADVERSARY.md](architecture/PERSISTENCE_ADVERSARY.md)
explains why each check is there).

```sh
cargo run --release -p openmhs-engine --example journal_startup -- generate J.sqlite 10000 10
# Append 1,000 writes left waiting for approval: unfinished commands for recovery to visit.
cargo run --release -p openmhs-engine --example journal_startup -- generate J.sqlite 0 0 1000
cargo run --release -p openmhs-engine --example journal_startup -- open J.sqlite
```

`generate` drives the real engine with a simulated supply: N verified writes, each followed
by R audited reads (each read writes an audit record, so reads, not commands, make most of a
busy journal). It turns fsync off to finish in minutes; the file is the same. `open` opens the
journal as a gateway does, in a fresh process, and times each step. Peak memory is the
process's peak working set.

### Measured

2026-10-31, commit after `4c5ac0b`, release build, Windows 11, Intel Core i7-10700 (16 logical
CPUs), 16 GB, NVMe SSD. Three opens of each journal in fresh processes; the medians, with the
file already in the operating system's cache.

| Journal | File | Audit records | Open (all checks) | quick_check | integrity_check | Audit chain | Recovery | Peak memory |
|---|---|---|---|---|---|---|---|---|
| 1,000 writes × 10 reads | 9.6 MB | 17 k | 0.06 s | 0.02 s | < 0.01 s | 0.04 s | 0.01 s | 15 MB |
| 10,000 × 10 | 96 MB | 170 k | 0.62 s | 0.18 s | 0.05 s | 0.41 s | 0.15 s | 16 MB |
| 10,000 × 100 | 472 MB | 1.07 M | 3.4 s | 0.83 s | 0.05 s | 2.6 s | 0.14 s | 15 MB |
| 100,000 × 10 | 964 MB | 1.70 M | 6.3 s | 1.6 s | 0.54 s | 4.1 s | 1.4 s | 16 MB |

A gateway's start-up cost is the open plus recovery: about 7.7 s for the 1 GB journal, warm,
at the time of this table. Recovery then parsed every receipt ever stored.

**Recovery now follows live state** ([RFC 0002](rfcs/0002-storage-engine.md), milestone 1,
measured old against new in one session, five opens each): on the 100k journal it fell from
1.21 s to 1.1 ms with nothing unfinished, and costs about 50 µs per unfinished command
(48 ms for 1,000, 0.52 s for 10,000) whatever the history. The open itself did not change
(6.6 s before, 6.8 s after, within run-to-run spread). Full table in the RFC.

**The first measurement changed the code.** The audit chain was first verified by loading
the whole log into memory: peak memory equalled the journal's size (969 MB for the 1 GB
journal, 724 MB for the read-heavy one), enough to stop a gateway starting on a Raspberry Pi.
It now streams the records (memory constant, about 15 MB at every size) and hashes them
without building each record's string first (2.4 times faster; the hashes are byte for byte
the same, pinned by `audit_hash_is_unchanged_from_the_joined_string`). Before, the 1 GB
journal opened in 11.9 s.

**A first open is slower.** The first open of the 1 GB journal after other large files had
been read took 21 s, and of the 472 MB one 10.6 s: much of the file came from disk, which
every check reads. This is not a controlled cold-cache measurement (Windows offers no simple
way to empty its cache), but a first start after a reboot will look more like it.

**What follows.** About 2.4 µs per audit record for the chain, and quick_check about 1.7 s
per GB, on this machine. A gateway that writes 1,000 audit records an hour reaches 1.7
million in about ten weeks. Two things bound the start-up: the journal limit (ADR-0021;
`--journal-max-mib 256` keeps the warm open near 2 s here), and archiving old records, which
[RFC 0002](rfcs/0002-storage-engine.md) stages and which is not built. A Raspberry Pi is several times slower than
this machine per core and was not measured.

## Not measured yet

- Linux and macOS.
- A real device over a real network.
- Startup and sweep time with N offline devices, in wall-clock time (the tests measure
  them in virtual time).
- Memory, except at journal start-up.
- More than one gateway.
- Journal start-up on a Raspberry Pi, and from a genuinely cold cache.
