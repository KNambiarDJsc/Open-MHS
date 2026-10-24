//! Process loss and protocol failures are contained (docs/architecture/adapter-host.md).
//!
//! Each test runs tests/adapters/misbehave.py in one misbehaving mode, through the real
//! host, over a real child process.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use openmhs_adapter::{AdapterCommand, AdapterErrorKind, HardwareAdapter, Health, Progress};
use openmhs_engine::Engine;
use openmhs_host::{HostOptions, ProcessAdapter, Status};
use openmhs_policy::{Effect, Grant, Policy};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Clock, CommandState, SystemClock};
use serde_json::json;

const RESTART_WITHIN: Duration = Duration::from_secs(15);

async fn start(mode: &str, extra: serde_json::Value) -> ProcessAdapter {
    ProcessAdapter::start(common::misbehave(mode, extra), HostOptions::default())
        .await
        .unwrap_or_else(|e| panic!("start {mode}: {e}"))
}

async fn restarted(adapter: &ProcessAdapter, generation: u64) -> Status {
    adapter
        .wait_for_status(RESTART_WITHIN, |s| {
            matches!(s, Status::Running { generation: g } if *g >= generation)
                || matches!(s, Status::Quarantined { .. })
        })
        .await
}

fn dose(deadline_ms: u64) -> AdapterCommand {
    AdapterCommand {
        command_id: "cmd-1".into(),
        capability: "dose.run".into(),
        operation: Operation::Invoke,
        value: None,
        params: BTreeMap::new(),
        idempotent: false,
        deadline: SystemClock.now().plus_ms(deadline_ms),
    }
}

fn marker() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("marker");
    (dir, path)
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_to_stdout_do_not_corrupt_the_protocol() {
    let adapter = start("normal", json!({})).await;
    for _ in 0..20 {
        let v = adapter.read("level.measured").await.unwrap();
        assert_eq!(
            v.value,
            serde_json::from_value(json!({"value": 1.5, "unit": "V"})).unwrap()
        );
    }
    assert_eq!(adapter.health().await, Health::Healthy);
    assert_eq!(adapter.status(), Status::Running { generation: 1 });
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_during_a_read_is_contained_and_the_process_restarted() {
    let adapter = start("crash_on_read", json!({})).await;
    let err = adapter.read("level.measured").await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected, "{err}");
    assert!(err.message.contains("exited"), "{err}");
    assert_eq!(
        restarted(&adapter, 2).await,
        Status::Running { generation: 2 }
    );
    assert_eq!(adapter.health().await, Health::Healthy);
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_after_dispatch_is_unknown_outcome_and_never_retried() {
    let (_dir, marker) = marker();
    let adapter = Arc::new(
        start(
            "crash_after_apply",
            json!({"config": {"marker": marker.display().to_string()}}),
        )
        .await,
    );
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let policy = Policy {
        grants: vec![Grant {
            id: "test".into(),
            principal: "test:agent".into(),
            device: "*".into(),
            capability: "*".into(),
            operations: vec![Operation::Read, Operation::Invoke],
            effect: Effect::Allow,
            max_side_effect: None,
            reason: None,
        }],
        approvers: vec![],
    };
    let engine = Engine::in_memory(clock, policy).unwrap();
    engine.register(adapter.clone()).await.unwrap();
    let receipt = engine
        .submit(
            "test:agent",
            &json!({
                "target": {"device": "test.misbehave-01", "capability": "dose.run"},
                "operation": "invoke",
                "params": {},
            }),
        )
        .await
        .unwrap();
    assert_eq!(receipt.state, CommandState::UnknownOutcome, "{receipt:?}");

    assert_eq!(
        restarted(&adapter, 2).await,
        Status::Running { generation: 2 }
    );
    // Give a retry every chance to happen; none may.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let deliveries = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(deliveries.lines().count(), 1, "{deliveries}");
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hung_process_is_killed_and_its_command_is_uncertain() {
    let adapter = start("hang_on_execute", json!({})).await;
    let (progress, _rx) = Progress::channel();
    let started = std::time::Instant::now();
    let err = adapter.execute(dose(30_000), progress).await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected, "{err}");
    assert!(err.kind.outcome_uncertain());
    assert!(err.message.contains("heartbeats"), "{err}");
    // Three missed 500 ms heartbeats, not the 30 s deadline.
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        restarted(&adapter, 2).await,
        Status::Running { generation: 2 }
    );
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_output_kills_the_process() {
    let adapter = start("garbage_on_read", json!({})).await;
    let err = adapter.read("level.measured").await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected, "{err}");
    assert!(err.message.contains("protocol violation"), "{err}");
    assert_eq!(
        restarted(&adapter, 2).await,
        Status::Running { generation: 2 }
    );
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_from_the_adapter_are_refused_without_harm() {
    let adapter = start("child_request", json!({})).await;
    adapter.read("level.measured").await.unwrap();
    assert_eq!(adapter.status(), Status::Running { generation: 1 });
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_protocol_version_is_refused() {
    let mut resolved = common::misbehave("normal", json!({}));
    resolved.args.extend(["--versions".into(), "9.9".into()]);
    let err = ProcessAdapter::start(resolved, HostOptions::default())
        .await
        .err()
        .expect("must not start");
    assert!(
        err.contains("none of the offered protocol versions"),
        "{err}"
    );
    assert!(err.contains("9.9"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_descriptor_beyond_the_manifest_is_refused() {
    let resolved = common::misbehave(
        "normal",
        json!({"capabilities": {"allow": [{"name": "level.*", "operations": ["read"]}]}}),
    );
    let err = ProcessAdapter::start(resolved, HostOptions::default())
        .await
        .err()
        .expect("must not start");
    assert!(err.contains("manifest violation"), "{err}");
    assert!(err.contains("dose.run"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_outside_the_manifest_never_reach_the_process() {
    let adapter = start("normal", json!({})).await;
    let err = adapter.read("secrets.dump").await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Unsupported);
    assert!(err.message.contains("manifest"), "{err}");
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_loop_is_quarantined() {
    let (_dir, marker) = marker();
    let adapter = start(
        "crash_loop",
        json!({"config": {"marker": marker.display().to_string()}}),
    )
    .await;
    // From now on every new process dies while initializing.
    std::fs::write(&marker, "").unwrap();
    assert!(adapter.read("level.measured").await.is_err());
    let status = restarted(&adapter, 99).await;
    let Status::Quarantined { reason } = status else {
        panic!("expected quarantine, got {status:?}");
    };
    assert!(reason.contains("crash loop"), "{reason}");

    let (progress, _rx) = Progress::channel();
    let err = adapter.execute(dose(5_000), progress).await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::NotSent, "{err}");
    assert!(matches!(adapter.health().await, Health::Disconnected(m) if m.contains("quarantined")));
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_descriptor_after_restart_is_quarantined() {
    let (_dir, marker) = marker();
    let adapter = start(
        "descriptor_changes",
        json!({"config": {"marker": marker.display().to_string()}}),
    )
    .await;
    std::fs::write(&marker, "").unwrap();
    assert!(adapter.read("level.measured").await.is_err());
    let status = restarted(&adapter, 99).await;
    let Status::Quarantined { reason } = status else {
        panic!("expected quarantine, got {status:?}");
    };
    assert!(reason.contains("different descriptor"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environment_is_scrubbed_and_credentials_passed() {
    // Cargo sets these for test processes.
    assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
    assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
    let adapter = start(
        "normal",
        json!({
            "credentials": ["CARGO_PKG_NAME"],
            "config": {"probe_env": ["CARGO_MANIFEST_DIR", "CARGO_PKG_NAME", "PYTHONPATH"]},
        }),
    )
    .await;
    let d = adapter.describe().await.unwrap();
    let seen: serde_json::Value =
        serde_json::from_str(d["device"]["description"].as_str().unwrap()).unwrap();
    assert_eq!(
        seen,
        json!({"CARGO_MANIFEST_DIR": false, "CARGO_PKG_NAME": true, "PYTHONPATH": true})
    );
    adapter.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_credential_refuses_to_start() {
    let resolved = common::misbehave(
        "normal",
        json!({"credentials": ["OPENMHS_TEST_CREDENTIAL_THAT_IS_NOT_SET"]}),
    );
    let err = ProcessAdapter::start(resolved, HostOptions::default())
        .await
        .err()
        .expect("must not start");
    assert!(
        err.contains("OPENMHS_TEST_CREDENTIAL_THAT_IS_NOT_SET"),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_is_final() {
    let adapter = start("normal", json!({})).await;
    adapter.shutdown().await.unwrap();
    assert_eq!(adapter.status(), Status::Stopped);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(adapter.status(), Status::Stopped);
    let err = adapter.read("level.measured").await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected);
}
