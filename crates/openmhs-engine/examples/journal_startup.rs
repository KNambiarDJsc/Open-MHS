//! How long a gateway takes to open its journal, as the journal grows. Startup checks every
//! journal before use (SQLite's `quick_check`, the full `integrity_check` of the commands and
//! approvals tables, the whole audit chain), then recovery scans the commands. This times
//! each step on journals made by the real engine.
//!
//! ```sh
//! # A journal of N verified writes, each followed by R audited reads, then L writes left
//! # waiting for approval (unfinished: recovery visits them, and they stay unfinished), through
//! # the engine and a simulated supply. Appends to J if it exists. Generation turns off fsync
//! # to finish in minutes; the file is the same as one written with it on.
//! cargo run --release -p openmhs-engine --example journal_startup -- generate J.sqlite N R [L]
//! # Open it as a gateway would, in a fresh process, and print the timings as JSON.
//! cargo run --release -p openmhs-engine --example journal_startup -- open J.sqlite
//! ```
//!
//! Results and method: docs/benchmarks.md, "Journal start-up".

#![allow(clippy::unwrap_used, clippy::print_stdout)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openmhs_adapter::TokioClock;
use openmhs_engine::{Engine, EngineOptions, Journal};
use openmhs_policy::{Caller, Effect, Grant, Policy};
use openmhs_sim::VirtualPsu;
use openmhs_types::descriptor::Operation;
use openmhs_types::{Clock, Timestamp};
use rusqlite::Connection;
use serde_json::json;

const PSU: &str = "sim.psu-01";
const AGENT: &str = "agent:bench";
/// Its writes wait for an approval that never comes.
const HELD: &str = "agent:held";
/// Generated time starts here; opening uses it too, so no approval expires on the wall clock.
const EPOCH_MS: i64 = 1_800_000_000_000;

fn policy() -> Policy {
    Policy {
        grants: vec![
            Grant {
                id: "bench".into(),
                principal: AGENT.into(),
                device: "*".into(),
                capability: "*".into(),
                operations: vec![Operation::Read, Operation::Write, Operation::Invoke],
                effect: Effect::Allow,
                max_side_effect: None,
                reason: None,
            },
            Grant {
                id: "held".into(),
                principal: HELD.into(),
                device: "*".into(),
                capability: "*".into(),
                operations: vec![Operation::Write],
                effect: Effect::RequireApproval,
                max_side_effect: None,
                reason: None,
            },
        ],
        approvers: vec![],
    }
}

fn generate(path: &Path, commands: u64, reads: u64, live: u64) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    rt.block_on(async {
        let clock: Arc<dyn Clock> =
            Arc::new(TokioClock::starting_at(Timestamp::from_millis(EPOCH_MS)));
        let journal = Journal::open(path).unwrap();
        // Generation only: the bytes written are the same, they are just not synced.
        journal.execute_raw("PRAGMA synchronous = OFF").unwrap();
        let engine =
            Engine::with_journal(journal, clock.clone(), policy(), EngineOptions::default());
        let (psu, _) = VirtualPsu::new(clock, 42);
        engine.register(Arc::new(psu)).await.unwrap();
        let agent = Caller::in_process(AGENT);
        // Keys unique to this run: generating into an existing journal appends.
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let started = Instant::now();
        for i in 0..commands {
            let volts = if i % 2 == 0 { 1.0 } else { 1.5 };
            let r = engine
                .submit(
                    &agent,
                    &json!({
                        "target": { "device": PSU, "capability": "power.output.voltage.setpoint" },
                        "operation": "write",
                        "value": { "value": volts, "unit": "V" },
                        "idempotency_key": format!("bench-{run}-{i}"),
                    }),
                )
                .await
                .unwrap();
            assert_eq!(r.state, openmhs_types::CommandState::Verified, "{r:?}");
            for _ in 0..reads {
                engine
                    .read(&agent, PSU, "power.output.voltage.measured")
                    .await
                    .unwrap();
            }
            // Clear the supply's rate and slew windows (virtual time).
            tokio::time::sleep(Duration::from_secs(2)).await;
            if (i + 1) % 10_000 == 0 {
                eprintln!(
                    "{} commands, {:.0} s",
                    i + 1,
                    started.elapsed().as_secs_f64()
                );
            }
        }
        let held = Caller::in_process(HELD);
        for i in 0..live {
            let r = engine
                .submit(
                    &held,
                    &json!({
                        "target": { "device": PSU, "capability": "power.output.voltage.setpoint" },
                        "operation": "write",
                        "value": { "value": 1.0, "unit": "V" },
                        "idempotency_key": format!("live-{run}-{i}"),
                    }),
                )
                .await
                .unwrap();
            assert_eq!(
                r.state,
                openmhs_types::CommandState::AwaitingApproval,
                "{r:?}"
            );
        }
    });
}

fn ms(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}

fn check(conn: &Connection, pragma: &str) {
    let rows: Vec<String> = conn
        .prepare(pragma)
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, ["ok"], "{pragma}");
}

fn open(path: &Path, linger_ms: u64) {
    let size = std::fs::metadata(path).unwrap().len();
    // What a gateway does: open the journal (every check), then recover.
    let t = Instant::now();
    let journal = Journal::open(path).unwrap();
    let open_ms = ms(t);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let (recover_ms, audit) = rt.block_on(async {
        let clock: Arc<dyn Clock> =
            Arc::new(TokioClock::starting_at(Timestamp::from_millis(EPOCH_MS)));
        let records = journal.head_checkpoint().unwrap().seq;
        let engine =
            Engine::with_journal(journal, clock.clone(), policy(), EngineOptions::default());
        let (psu, _) = VirtualPsu::new(clock, 42);
        engine.register(Arc::new(psu)).await.unwrap();
        let t = Instant::now();
        let report = engine.recover().await.unwrap();
        let recover_ms = ms(t);
        assert!(report.not_executed.is_empty(), "{report:?}");
        (recover_ms, records)
    });

    // Each check on its own, after the above (the file is in the OS cache by now).
    let conn = Connection::open(path).unwrap();
    let commands: i64 = conn
        .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
        .unwrap();
    let unfinished: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commands WHERE state = 'AWAITING_APPROVAL'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let t = Instant::now();
    check(&conn, "PRAGMA quick_check");
    let quick_ms = ms(t);
    let t = Instant::now();
    check(&conn, "PRAGMA integrity_check(commands)");
    check(&conn, "PRAGMA integrity_check(approvals)");
    let integrity_ms = ms(t);
    drop(conn);
    let t = Instant::now();
    let journal = Journal::open(path).unwrap();
    let reopen_ms = ms(t);
    let t = Instant::now();
    assert!(journal.verify_audit().unwrap().is_valid());
    let chain_ms = ms(t);

    println!(
        "{}",
        json!({
            "file_bytes": size, "commands": commands, "unfinished": unfinished,
            "audit_records": audit,
            "open_ms": open_ms, "recover_ms": recover_ms,
            "reopen_ms": reopen_ms, "quick_check_ms": quick_ms,
            "integrity_check_ms": integrity_ms, "audit_chain_ms": chain_ms,
        })
    );
    // Let a parent read this process's peak memory before it exits.
    std::thread::sleep(Duration::from_millis(linger_ms));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["generate", path, n, r] => {
            generate(Path::new(path), n.parse().unwrap(), r.parse().unwrap(), 0)
        }
        ["generate", path, n, r, l] => generate(
            Path::new(path),
            n.parse().unwrap(),
            r.parse().unwrap(),
            l.parse().unwrap(),
        ),
        ["open", path] => open(Path::new(path), 0),
        ["open", path, linger] => open(Path::new(path), linger.parse().unwrap()),
        _ => {
            eprintln!(
                "usage: journal_startup generate PATH COMMANDS READS [LIVE] | open PATH [LINGER_MS]"
            );
            std::process::exit(2);
        }
    }
}
