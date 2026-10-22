//! The engineering invariants from the master plan, one or more tests each.
//! Numbering follows docs/roadmap.md / the master build prompt §6.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use common::*;
use openmhs_engine::{Engine, EngineError};
use openmhs_policy::{Effect, Policy};
use openmhs_sim::{ExecFault, Faults, VirtualPsu};
use openmhs_types::descriptor::Operation;
use openmhs_types::{CommandState, Outcome};
use serde_json::json;

// 1. A discovered device is not automatically writable.
#[tokio::test(start_paused = true)]
async fn inv01_discovered_device_is_not_writable() {
    let (engine, psu) = setup(Policy::default()).await;
    assert_eq!(
        engine.devices().len(),
        1,
        "device is registered (discovered)"
    );

    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Denied);
    assert_eq!(codes(&r), ["policy.denied.no_grant"]);
    assert_eq!(psu.execute_calls(), 0);

    let read = engine.read(AGENT, PSU, SETPOINT).await;
    assert!(matches!(read, Err(EngineError::Denied(e)) if e.code == "policy.denied.no_grant"));
    assert_schema_valid(&r);
}

// 2. Unauthorized commands never reach the adapter — not even for state reads.
#[tokio::test(start_paused = true)]
async fn inv02_unauthorized_commands_never_reach_adapter() {
    let read_only = Policy {
        grants: vec![grant("grant.read", "*", &[Operation::Read], Effect::Allow)],
        approvers: vec![],
    };
    let engine = Engine::in_memory(clock(), read_only).unwrap();
    let (trap, reads, executes) = TrapAdapter::new();
    engine.register(Arc::new(trap)).await.unwrap();

    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Denied);

    let mut explicit = Policy::default();
    explicit
        .grants
        .push(grant("grant.all", "*", &[Operation::Write], Effect::Allow));
    explicit.grants.push(grant(
        "deny.setpoint",
        SETPOINT,
        &[Operation::Write],
        Effect::Deny,
    ));
    engine.set_policy(explicit).unwrap();
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["policy.denied.explicit"]);

    assert_eq!(reads.load(Ordering::SeqCst), 0, "no device traffic at all");
    assert_eq!(executes.load(Ordering::SeqCst), 0);
}

// 3. Invalid units, out-of-range values and schema violations are rejected before dispatch.
#[tokio::test(start_paused = true)]
async fn inv03_invalid_values_rejected_before_dispatch() {
    let (engine, psu) = setup(allow_all()).await;
    let cases = [
        (volts(48.0), "command.value.out_of_range"),
        (
            json!({ "value": 5, "unit": "volts" }),
            "command.value.unit_unknown",
        ),
        (
            json!({ "value": 5, "unit": "A" }),
            "command.value.unit_mismatch",
        ),
        (json!(5), "command.value.type_mismatch"),
        (json!(true), "command.value.type_mismatch"),
    ];
    for (value, code) in cases {
        let r = engine
            .submit(AGENT, &write(SETPOINT, value.clone()))
            .await
            .unwrap();
        assert_eq!(r.state, CommandState::Denied, "{value}");
        assert_eq!(codes(&r), [code], "{value}");
        assert_schema_valid(&r);
    }
    let bad = json!({ "target": { "device": PSU, "capability": SETPOINT }, "operation": "write" });
    assert!(matches!(
        engine.submit(AGENT, &bad).await,
        Err(EngineError::InvalidRequest(_))
    ));

    assert_eq!(psu.execute_calls(), 0);
    assert_eq!(
        psu.read_calls(),
        0,
        "stateless failures never touch the device"
    );
    assert_eq!(psu.truth().applied_count, 0);
}

// 4. Stale telemetry blocks operations that require fresh telemetry.
#[tokio::test(start_paused = true)]
async fn inv04_stale_or_bad_telemetry_blocks_commands() {
    let (engine, psu) = setup(allow_all()).await;
    for faults in [
        Faults {
            stale_by_ms: 5_000,
            ..Faults::default()
        },
        Faults {
            bad_quality: true,
            ..Faults::default()
        },
        Faults {
            disconnected: true,
            ..Faults::default()
        },
    ] {
        psu.set_faults(faults.clone());
        let r = engine
            .submit(AGENT, &write(SETPOINT, volts(1.0)))
            .await
            .unwrap();
        assert_eq!(r.state, CommandState::Denied, "{faults:?}");
        assert!(
            codes(&r).iter().all(|c| c == "command.state.stale"),
            "{faults:?}: {:?}",
            codes(&r)
        );
    }
    assert_eq!(psu.execute_calls(), 0);

    psu.clear_faults();
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Verified);
}

// 5. Duplicate or retried submissions cannot cause duplicate dispatches.
#[tokio::test(start_paused = true)]
async fn inv05_duplicates_dispatch_once() {
    let (engine, psu) = setup(allow_all()).await;
    let req = with(
        write(SETPOINT, volts(1.0)),
        "idempotency_key",
        json!("step-1"),
    );

    let first = engine.submit(AGENT, &req).await.unwrap();
    let second = engine.submit(AGENT, &req).await.unwrap();
    assert_eq!(first.command_id, second.command_id);
    assert_eq!(second.state, CommandState::Verified);
    assert_eq!(psu.execute_calls(), 1);
    assert_eq!(psu.truth().applied_count, 1);

    let changed = with(
        write(SETPOINT, volts(1.5)),
        "idempotency_key",
        json!("step-1"),
    );
    let err = engine.submit(AGENT, &changed).await.unwrap_err();
    assert_eq!(err.code(), "command.idempotency.conflict");
    assert_eq!(psu.execute_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn inv05_concurrent_duplicates_dispatch_once() {
    let (engine, psu) = setup(allow_all()).await;
    let req = with(
        write(SETPOINT, volts(1.0)),
        "idempotency_key",
        json!("burst"),
    );
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let (e, r) = (engine.clone(), req.clone());
            tokio::spawn(async move { e.submit(AGENT, &r).await.unwrap() })
        })
        .collect();
    let mut ids = std::collections::HashSet::new();
    for t in tasks {
        ids.insert(t.await.unwrap().command_id);
    }
    assert_eq!(ids.len(), 1);
    assert_eq!(psu.execute_calls(), 1);
}

// 6. A timeout is never treated as failure, and is never retried automatically.
#[tokio::test(start_paused = true)]
async fn inv06_timeout_is_uncertain_and_not_retried() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let r = engine
        .submit(
            AGENT,
            &with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(3_000)),
        )
        .await
        .unwrap();

    assert_eq!(r.state, CommandState::UnknownOutcome);
    assert_eq!(r.outcome, Outcome::Uncertain);
    assert!(states(&r).contains(&CommandState::TimedOut));
    assert!(!states(&r).contains(&CommandState::Failed));
    assert_eq!(psu.execute_calls(), 1, "never retried");
    assert_schema_valid(&r);
}

// 7. A crash mid-command is recovered into UNKNOWN_OUTCOME on restart.
#[tokio::test(start_paused = true)]
async fn inv07_crash_mid_dispatch_recovers_to_unknown_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.db");

    let engine = Engine::open(&path, clock(), allow_all()).unwrap();
    let (psu, handle) = VirtualPsu::new(clock(), 1);
    handle.update_faults(|f| f.execute = Some(ExecFault::Hang));
    engine.register(Arc::new(psu)).await.unwrap();

    let e = engine.clone();
    let task = tokio::spawn(async move { e.submit(AGENT, &write(SETPOINT, volts(1.0))).await });
    let id = loop {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let in_flight = engine
            .journal()
            .commands_in_states(&[CommandState::Dispatched])
            .unwrap();
        if let Some(r) = in_flight.first() {
            break r.command_id.clone();
        }
    };
    // "Crash": the gateway stops while the command is dispatched.
    task.abort();
    let _ = task.await;
    drop(engine);

    let restarted = Engine::open(&path, clock(), allow_all()).unwrap();
    let (psu2, _) = VirtualPsu::new(clock(), 1);
    restarted.register(Arc::new(psu2)).await.unwrap();
    let report = restarted.recover().await.unwrap();

    assert_eq!(report.unknown_outcome, vec![id.clone()]);
    let r = restarted.receipt(&id).unwrap().unwrap();
    assert_eq!(r.state, CommandState::UnknownOutcome);
    assert!(r.last_detail().unwrap().contains("restarted"));
    assert!(restarted.verify_audit().unwrap().is_valid());
}

// 8. Device-specific constraints stay out of the core. Heuristic: the non-test source of
// the core crates must not mention device categories. Fixtures and tests may.
#[test]
fn inv08_core_has_no_device_specific_code() {
    let banned = [
        "psu",
        "power supply",
        "voltage",
        "thermo",
        "axis",
        "robot",
        "pump",
        "actuator",
        "sensor",
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for krate in [
        "openmhs-engine",
        "openmhs-policy",
        "openmhs-adapter",
        "openmhs-types",
    ] {
        for entry in std::fs::read_dir(root.join(krate).join("src")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap().to_lowercase();
            for word in banned {
                if code.contains(word) {
                    offenders.push(format!("{}: '{word}'", path.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "device-specific terms in core code:\n{}",
        offenders.join("\n")
    );
}

// 11. Every command produces a traceable outcome, including denials and uncertainty.
#[tokio::test(start_paused = true)]
async fn inv11_every_command_has_receipt_and_audit_trail() {
    let (engine, psu) = setup(allow_all()).await;
    let mut receipts = vec![
        engine
            .submit(AGENT, &write(SETPOINT, volts(1.0)))
            .await
            .unwrap(),
        engine
            .submit(AGENT, &write(SETPOINT, volts(99.0)))
            .await
            .unwrap(),
        engine
            .submit("agent.stranger", &write(SETPOINT, volts(1.0)))
            .await
            .unwrap(),
    ];
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    receipts.push(
        engine
            .submit(
                AGENT,
                &with(write(SETPOINT, volts(1.2)), "deadline_ms", json!(500)),
            )
            .await
            .unwrap(),
    );

    let audit = engine.journal().audit_records().unwrap();
    for r in &receipts {
        assert_schema_valid(r);
        assert!(
            r.state.is_terminal() || r.state == CommandState::UnknownOutcome,
            "{:?}",
            r.state
        );
        let trail: Vec<_> = audit.iter().filter(|a| a.subject == r.command_id).collect();
        assert_eq!(
            trail.len(),
            r.transitions.len(),
            "one audit record per transition for {}",
            r.command_id
        );
        assert_eq!(
            r.audit_hash.as_deref(),
            Some(trail.last().unwrap().hash.as_str())
        );
    }
}

// 12. Missing policy, limits or telemetry fail closed. (No-grant and stale cases are
// covered above; here: a precondition whose source cannot be read.)
#[tokio::test(start_paused = true)]
async fn inv12_unreadable_precondition_fails_closed() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.disconnected = true);
    let r = engine
        .submit(AGENT, &write(ENABLED, json!(true)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Denied);
    assert_eq!(codes(&r), ["command.state.stale"]);
    assert_eq!(
        r.policy.errors[0].rule_id.as_deref(),
        Some("protection-not-tripped")
    );
}

// 14. Audit records are tamper-evident.
#[tokio::test(start_paused = true)]
async fn inv14_audit_chain_verifies_and_detects_tampering() {
    let (engine, _psu) = setup(allow_all()).await;
    engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    engine.read(AGENT, PSU, SETPOINT).await.unwrap();
    let v = engine.verify_audit().unwrap();
    assert!(v.is_valid() && v.records > 5);

    engine.journal().execute_raw("UPDATE audit SET data = replace(data, 'VERIFIED', 'FAILED') WHERE kind = 'command.transition'").unwrap();
    assert!(!engine.verify_audit().unwrap().is_valid());
}
