# Recorded run of the adapter guide

The M5 acceptance criteria ask that a fresh adapter can be created, built and
conformance-tested by following only [the guide](README.md). This is the record of
doing that on 2026-10-23.

**Environment:** Windows 11, Git Bash, rustc 1.99.1. The `openmhs` binary came from the
local checkout on branch `m5` (section 2 of the guide). A fresh `git clone` was not used,
because the M5 code was not yet on GitHub. The adapter was created in an empty directory
outside the checkout.

| Step | Guide section | Command or change | Result |
|---|---|---|---|
| 1 | 3 | `openmhs adapter new bench-lamp --openmhs-path C:\Users\91704\openmhs` in an empty directory | "Created bench-lamp (9 files)". `Cargo.toml` names the three OpenMHS crates and `openmhs-gateway` by path, and has its own `[workspace]` |
| 2 | 4 | `cd bench-lamp && cargo test` | First build 4 min 24 s. `reaches_its_declared_level ... ok`, `reaches_its_declared_level_over_tcp ... ok` (1.99 s). Reports in `target/tmp/conformance/` |
| 3 | 6.1, 6.2 | The descriptor entry and the two `lib.rs` arms, copied from the guide | Compiles |
| 4 | 6.3 | `cargo test` | Fails as the guide shows: `reached L0, declared L2 -> FAIL`. 2 fail (`L1.read`, `L2.command` on `lamp.color_temp`, both naming the reply `ERR unknown query`), 10 untested, each pointing at one of the two failures |
| 5 | 6.4 | The four `fixture.rs` snippets, copied from the guide | Compiles |
| 6 | 6.5 | `cargo test` | Both pass. `tcp.json`: achieved L2, 60 pass, 0 fail, 1 untested (`L3.leases`), 4 not applicable. Every `lamp.color_temp` check passed, including `L2.lost_ack` ("reconciled as applied by reading back") |
| 7 | 8 | `cargo build --examples`, then `cargo run --example serve -- --home ./home` | "gateway at http://127.0.0.1:55944" |
| 8 | 8 | `openmhs --home ./home devices list` | `bench-lamp-01  Example lamp  bench-lamp  6 capabilities` |
| 9 | 8 | `read` before any grant | Denied: `policy.denied.no_grant` (deny by default, as the guide says) |
| 10 | 8 | `grants add ... --ops read,write,invoke`, then `read bench-lamp-01 lamp.color_temp` | `4000 K (GOOD, simulator, 1 ms old)` |
| 11 | 8 | `exec bench-lamp-01 lamp.color_temp --value 3000K` | `VERIFIED`, readback 3000 K, exit 0 |
| 12 | 8 | `--value 9000K` | `DENIED command.value.out_of_range`, nothing sent, exit 2 |
| 13 | 8 | `exec bench-lamp-01 lamp.flash --param count=2` with the lamp off | `DENIED command.precondition.failed: A lamp that is off cannot flash.`, exit 2 |
| 14 | 8 | Lamp on, then flash again | `UNVERIFIABLE` (verification mode `none`), exit 6. `lamp.flash.count` reads 2 |
| 15 | — | `openmhs --home ./home audit verify` | "audit log intact: 35 records, hash chain verified" |

The edits in steps 3 and 5 were made with a script that inserted the guide's snippets
verbatim at the places the guide names.

## What the run changed

Following the guide exposed two problems, both fixed before this record was final:

- **Cascading failures.** In step 4 the suite first reported five failures for one
  missing protocol command. Fault checks that depend on a working command
  (`L2.malformed`, `L2.lost_ack`, ...) failed with advice that did not apply ("return
  MalformedResponse"), and `L1.reconnect` failed on a property that had never worked.
  Now those checks are `untested` with a pointer to the root cause, and the outage checks
  use only properties that read before the outage. A regression test covers it:
  `an_unimplemented_capability_is_blamed_once_not_cascaded` in
  `crates/openmhs-conformance/tests/broken.rs`.
- **CLI times.** Receipt history printed `17:00:09.62Z` for `.620`, and a whole RFC 3339
  timestamp for `.900`, because trailing zeros are trimmed. `clock_time` now computes
  `HH:MM:SS.mmm` itself, and a test covers it.
