//! Routine telemetry (RFC 0002, milestone 6): opted-in reads go to a store of their own and
//! are summarized in the audit chain; nothing else changes; loss is counted, never silent;
//! and no `Critical` or `Security` record is lost to quota pressure.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use openmhs_engine::telemetry::{TelemetryConfig, TelemetryRule, TelemetryStore};
use openmhs_engine::{Engine, EngineError, StorageLimits};
use openmhs_policy::{Caller, Effect, Grant, Policy};
use openmhs_sim::{AxisHandle, PsuHandle, VirtualAxis, VirtualPsu};
use openmhs_types::audit_class::{EventClass, class_of};
use openmhs_types::descriptor::Operation;
use openmhs_types::{CommandState, Timestamp};
use serde_json::{Value, json};

const MEASURED: &str = "power.output.voltage.measured";
const AXIS: &str = "sim.axis-01";
const MIB: u64 = 1024 * 1024;

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn config(soft: u64, hard: u64) -> TelemetryConfig {
    TelemetryConfig {
        rules: vec![TelemetryRule {
            device: "sim.psu-*".into(),
            capability: "power.output.voltage.*".into(),
        }],
        interval_ms: 60_000,
        soft_bytes: soft,
        hard_bytes: hard,
        retention_ms: 7 * 24 * 3_600_000,
    }
}

fn roomy() -> TelemetryConfig {
    config(64 * MIB, 128 * MIB)
}

fn kinds(engine: &Engine) -> Vec<String> {
    engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|r| r.kind)
        .collect()
}

fn count(engine: &Engine, kind: &str) -> usize {
    kinds(engine).iter().filter(|k| *k == kind).count()
}

fn summaries(engine: &Engine) -> Vec<Value> {
    engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == "read.summary")
        .map(|r| serde_json::from_str(&r.data).unwrap())
        .collect()
}

async fn with_telemetry(config: TelemetryConfig) -> (Engine, PsuHandle, tempfile::TempDir) {
    let (engine, psu) = setup(allow_all()).await;
    let dir = tempfile::tempdir().unwrap();
    engine
        .enable_telemetry(&dir.path().join("telemetry.sqlite"), config)
        .unwrap();
    (engine, psu, dir)
}

#[tokio::test(start_paused = true)]
async fn opted_in_reads_are_summarized_and_everything_else_stays_individual() {
    let (engine, psu, _dir) = with_telemetry(roomy()).await;
    psu.update_faults(|f| f.bad_quality = false);
    for _ in 0..5 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    // Not opted in: the enable flag, and a read of a capability no rule names.
    for _ in 0..3 {
        engine.read(&agent(), PSU, ENABLED).await.unwrap();
    }
    assert_eq!(
        count(&engine, "read"),
        3,
        "only the unopted reads are records"
    );
    assert_eq!(
        count(&engine, "read.summary"),
        0,
        "nothing is written before a flush"
    );
    assert_eq!(engine.telemetry_status().unwrap().unwrap().rows, 5);

    let report = engine.flush_telemetry(true).unwrap();
    assert_eq!((report.summaries, report.reads, report.dropped), (1, 5, 0));
    let all = summaries(&engine);
    let s = &all[0];
    assert_eq!(s["principal"], AGENT);
    assert_eq!(s["capability"], MEASURED);
    assert_eq!(
        (
            s["reads"].clone(),
            s["stored"].clone(),
            s["dropped"].clone()
        ),
        (5.into(), 5.into(), 0.into())
    );
    assert_eq!(s["quality"], json!({ "good": 5, "uncertain": 0, "bad": 0 }));
    let (min, max) = (
        s["min"]["value"].as_f64().unwrap(),
        s["max"]["value"].as_f64().unwrap(),
    );
    assert!(min <= max && s["min"]["unit"] == "V", "{s}");
    assert!(s["first"]["at_ms"].as_i64() <= s["last"]["at_ms"].as_i64());
    assert_eq!(
        s["store"]["id"],
        engine
            .telemetry_status()
            .unwrap()
            .unwrap()
            .store_id
            .as_str()
    );
    let record = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == "read.summary")
        .unwrap();
    assert_eq!(
        record.subject, PSU,
        "the summary is about the device, like a read"
    );
    assert!(engine.verify_audit().unwrap().is_valid());

    // A second flush has nothing new; reads after the first form the next summary.
    assert_eq!(engine.flush_telemetry(true).unwrap().summaries, 0);
    for _ in 0..2 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    assert_eq!(engine.flush_telemetry(true).unwrap().reads, 2);
    let all = summaries(&engine);
    assert_eq!(all.len(), 2);
    assert!(all[1]["store"]["from_id"].as_i64() > all[0]["store"]["to_id"].as_i64());
}

#[tokio::test(start_paused = true)]
async fn a_window_is_summarized_only_once_it_has_ended() {
    let (engine, _psu, _dir) = with_telemetry(roomy()).await;
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    // Inside the window: nothing yet, and nothing lost.
    assert_eq!(engine.flush_telemetry(false).unwrap().summaries, 0);
    tokio::time::sleep(Duration::from_millis(61_000)).await;
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    let report = engine.flush_telemetry(false).unwrap();
    assert_eq!(
        (report.summaries, report.reads),
        (1, 1),
        "the ended window only"
    );
    assert_eq!(engine.telemetry_status().unwrap().unwrap().rows, 2);
    assert_eq!(engine.flush_telemetry(true).unwrap().reads, 1);
}

#[tokio::test(start_paused = true)]
async fn denied_failed_and_bad_reads_are_not_hidden_in_a_summary() {
    // A principal with no read grant: the refusal is Security evidence, individually.
    let (engine, psu) = setup(Policy {
        grants: vec![],
        approvers: vec![OPERATOR.into()],
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    engine
        .enable_telemetry(&dir.path().join("t.sqlite"), roomy())
        .unwrap();
    for _ in 0..3 {
        assert!(engine.read(&agent(), PSU, MEASURED).await.is_err());
    }
    assert_eq!(count(&engine, "read.denied"), 3);
    assert_eq!(engine.telemetry_status().unwrap().unwrap().rows, 0);
    assert_eq!(engine.flush_telemetry(true).unwrap().summaries, 0);

    // A read that fails is an individual record too, though the device is opted in.
    let (engine, psu2, _dir2) = with_telemetry(roomy()).await;
    psu2.update_faults(|f| f.disconnected = true);
    assert!(engine.read(&agent(), PSU, MEASURED).await.is_err());
    assert_eq!(count(&engine, "read"), 1);
    assert_eq!(engine.telemetry_status().unwrap().unwrap().rows, 0);
    // A reading of bad quality is still a reading, and counted as bad.
    psu2.clear_faults();
    psu2.update_faults(|f| f.bad_quality = true);
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    engine.flush_telemetry(true).unwrap();
    assert_eq!(summaries(&engine)[0]["quality"]["bad"], 1);
    drop(psu);
}

#[tokio::test(start_paused = true)]
async fn at_the_hard_limit_raw_readings_stop_but_every_read_is_counted() {
    // A store with room for a few hundred readings only.
    let (engine, _psu, _dir) = with_telemetry(config(64 * 1024, 96 * 1024)).await;
    for _ in 0..600 {
        // The read itself is never refused for want of room in the telemetry store.
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    let before = engine.telemetry_status().unwrap().unwrap();
    assert!(before.dropped_pending > 0, "{before:?}");
    assert!(before.rows > 0 && before.rows < 600);

    let report = engine.flush_telemetry(true).unwrap();
    assert_eq!(report.reads, 600, "every read is in a summary");
    assert_eq!(report.dropped, before.dropped_pending);
    let s = &summaries(&engine)[0];
    assert_eq!(
        s["stored"].as_u64().unwrap() + s["dropped"].as_u64().unwrap(),
        600
    );
    assert!(
        s["dropped"].as_u64().unwrap() > 0,
        "the loss is stated: {s}"
    );
    // The counters are taken off once written.
    assert_eq!(
        engine.telemetry_status().unwrap().unwrap().dropped_pending,
        0
    );
    assert!(engine.verify_audit().unwrap().is_valid());
}

#[tokio::test(start_paused = true)]
async fn over_the_soft_limit_only_summarized_readings_are_removed() {
    let (engine, _psu, _dir) = with_telemetry(config(64 * 1024, MIB)).await;
    for _ in 0..400 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    engine.flush_telemetry(true).unwrap();
    let summarized = engine.telemetry_status().unwrap().unwrap().rows;
    for _ in 0..400 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    let status = engine.telemetry_status().unwrap().unwrap();
    assert!(
        status.rows < summarized + 400,
        "old readings made room: {status:?}"
    );
    // The 400 not yet summarized are all still there.
    let report = engine.flush_telemetry(true).unwrap();
    assert_eq!((report.reads, report.dropped), (400, 0));
    assert_eq!(summaries(&engine)[1]["stored"], 400);
}

#[tokio::test(start_paused = true)]
async fn summarized_readings_are_removed_after_their_retention() {
    let mut cfg = roomy();
    cfg.retention_ms = 1_000;
    let (engine, _psu, _dir) = with_telemetry(cfg).await;
    for _ in 0..5 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    assert_eq!(
        engine.flush_telemetry(true).unwrap().pruned,
        0,
        "still fresh"
    );
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(engine.flush_telemetry(true).unwrap().pruned, 5);
    assert_eq!(engine.telemetry_status().unwrap().unwrap().rows, 0);
    // The summary in the chain outlives the raw readings.
    assert_eq!(summaries(&engine).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_bad_configuration_is_refused() {
    let (engine, _psu) = setup(allow_all()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.sqlite");
    let mut none = roomy();
    none.rules.clear();
    let mut inverted = roomy();
    inverted.soft_bytes = inverted.hard_bytes;
    let mut instant = roomy();
    instant.interval_ms = 0;
    for bad in [none, inverted, instant] {
        let error = engine.enable_telemetry(&path, bad).unwrap_err();
        assert!(matches!(error, EngineError::InvalidRequest(_)), "{error:?}");
    }
    // Without telemetry nothing is routed and a flush is a no-op.
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    assert_eq!(count(&engine, "read"), 1);
    assert_eq!(engine.flush_telemetry(true).unwrap().summaries, 0);
    assert!(engine.telemetry_status().unwrap().is_none());
}

/// A crash after the chain committed a flush's summaries but before the store marked its
/// readings summarized must not summarize them twice.
#[tokio::test(start_paused = true)]
async fn a_crash_between_the_chain_and_the_store_does_not_summarize_twice() {
    let dir = tempfile::tempdir().unwrap();
    let (journal, telemetry) = (
        dir.path().join("journal.sqlite"),
        dir.path().join("t.sqlite"),
    );
    let engine = |path: &Path| {
        let clock = clock();
        let (psu, _) = VirtualPsu::new(clock.clone(), 42);
        (Engine::open(path, clock, allow_all()).unwrap(), psu)
    };
    {
        let (engine, psu) = engine(&journal);
        engine.register(Arc::new(psu)).await.unwrap();
        engine.enable_telemetry(&telemetry, roomy()).unwrap();
        for _ in 0..4 {
            engine.read(&agent(), PSU, MEASURED).await.unwrap();
        }
    }
    // The first half of a flush: the summaries reach the chain, the store is not told.
    {
        let store = TelemetryStore::open(&telemetry, &roomy()).unwrap();
        let plan = store.plan(0, 60_000, true).unwrap();
        assert_eq!(plan.summaries.len(), 1);
        let records: Vec<_> = plan
            .summaries
            .iter()
            .map(|s| {
                (
                    Timestamp::from_millis(1),
                    "read.summary".to_owned(),
                    s.device.clone(),
                    s.data.clone(),
                )
            })
            .collect();
        openmhs_engine::Journal::open(&journal)
            .unwrap()
            .append_audit_many(&records)
            .unwrap();
    }
    let (engine, psu) = engine(&journal);
    engine.register(Arc::new(psu)).await.unwrap();
    engine.enable_telemetry(&telemetry, roomy()).unwrap();
    assert_eq!(
        engine.flush_telemetry(true).unwrap().summaries,
        0,
        "already summarized"
    );
    assert_eq!(count(&engine, "read.summary"), 1);
    // New readings after the restart are summarized as usual.
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    assert_eq!(engine.flush_telemetry(true).unwrap().reads, 1);
}

// ------------------------------------------------------------ quota pressure

fn lab_policy() -> Policy {
    let grant = |id: &str, device: &str, ops: &[Operation]| Grant {
        id: id.into(),
        principal: AGENT.into(),
        device: device.into(),
        capability: "*".into(),
        operations: ops.to_vec(),
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    };
    Policy {
        grants: vec![
            grant("agent.psu", PSU, &[Operation::Read, Operation::Write]),
            grant("agent.axis", AXIS, &[Operation::Read, Operation::Invoke]),
        ],
        approvers: vec![OPERATOR.into()],
    }
}

async fn lab() -> (Engine, PsuHandle, AxisHandle) {
    let clock = clock();
    let (psu, psu_handle) = VirtualPsu::new(clock.clone(), 42);
    let (axis, axis_handle) = VirtualAxis::new(clock.clone());
    let engine = Engine::in_memory(clock, lab_policy()).unwrap();
    engine.register(Arc::new(psu)).await.unwrap();
    engine.register(Arc::new(axis)).await.unwrap();
    (engine, psu_handle, axis_handle)
}

/// The milestone's exit test. The journal is within its critical reserve and the telemetry
/// store is full: telemetry is the only thing that gives way, and it gives way explicitly.
/// Every `Critical` and `Security` record is still written, and no read is lost to a count.
#[tokio::test(start_paused = true)]
async fn no_critical_or_security_record_is_lost_under_quota_pressure() {
    let (engine, _psu, axis) = lab().await;
    let dir = tempfile::tempdir().unwrap();
    engine
        .enable_telemetry(&dir.path().join("t.sqlite"), config(64 * 1024, 96 * 1024))
        .unwrap();
    // The telemetry store fills up while the journal is healthy.
    for _ in 0..300 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    // Now the journal is within its reserve.
    let used = engine.journal().size_bytes().unwrap();
    engine
        .set_storage_limits(StorageLimits {
            max_bytes: used + MIB,
            reserve_bytes: MIB,
        })
        .unwrap();
    let records_before = engine.journal().audit_records().unwrap().len();

    // Telemetry reads still work and still cost the journal nothing; a summary does not
    // spend the reserve, it waits.
    for _ in 0..200 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    assert!(engine.flush_telemetry(true).unwrap().deferred);
    assert_eq!(
        engine.journal().audit_records().unwrap().len(),
        records_before
    );
    // An audited read of anything not opted in is refused, as before (ADR-0021).
    let refused = engine.read(&agent(), AXIS, "axis.position").await;
    assert!(matches!(&refused, Err(EngineError::Denied(e)) if e.code == "storage.exhausted"));

    // Critical: a stop is accepted and its lifecycle is recorded.
    let stopped = engine
        .submit(
            &agent(),
            &json!({ "target": { "device": AXIS, "capability": "axis.stop" },
                     "operation": "invoke", "params": {} }),
        )
        .await
        .unwrap();
    assert_ne!(stopped.state, CommandState::Denied, "{stopped:?}");
    assert_eq!(axis.execute_calls(), 1);
    // Security: a refused request is still evidence.
    let outsider = Caller::in_process("agent.outsider");
    assert!(engine.read(&outsider, PSU, MEASURED).await.is_err());
    let after = kinds(&engine);
    assert!(after.iter().any(|k| k == "command.created"), "{after:?}");
    assert!(after.iter().any(|k| k == "command.transition"));
    assert!(
        after.iter().any(|k| k == "read.denied"),
        "the refusal was recorded"
    );
    // Nothing written in the reserve was a routine record.
    for kind in &after[records_before..] {
        assert_ne!(class_of(kind), EventClass::Telemetry, "{kind}");
    }
    assert!(engine.verify_audit().unwrap().is_valid());

    // Space comes back: the summaries are written, and they account for every read made
    // under pressure, kept or dropped.
    let used = engine.journal().size_bytes().unwrap();
    engine
        .set_storage_limits(StorageLimits {
            max_bytes: used + 64 * MIB,
            reserve_bytes: MIB,
        })
        .unwrap();
    let report = engine.flush_telemetry(true).unwrap();
    assert!(!report.deferred);
    assert_eq!(report.reads, 500, "300 before the limit, 200 under it");
    assert!(
        report.dropped > 0,
        "the full store dropped some, and said so"
    );
    let total: u64 = summaries(&engine)
        .iter()
        .map(|s| s["reads"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 500);
    assert!(engine.verify_audit().unwrap().is_valid());
}
