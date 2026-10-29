//! Reopen each durable command boundary and prove that recovery never invokes a device.
#![allow(clippy::unwrap_used)]
mod common;
use common::*;
use openmhs_engine::{Engine, journal::Journal};
use openmhs_types::evidence::verify_evidence;
use openmhs_types::receipt::AdapterResult;
use openmhs_types::{CommandRequest, CommandState, Receipt};
use serde_json::json;
use std::sync::{Arc, atomic::Ordering};

#[tokio::test(start_paused = true)]
async fn restart_boundaries_preserve_uncertainty_and_native_reference_without_resending() {
    use CommandState::*;
    for stop_at in [
        Created,
        Validating,
        Approved,
        Dispatched,
        Accepted,
        TimedOut,
        Applied,
        UnknownOutcome,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.sqlite");
        let now = clock().now();
        let request: CommandRequest = serde_json::from_value(write(SETPOINT, volts(1.0))).unwrap();
        {
            let journal = Journal::open(&path).unwrap();
            let receipt = Receipt::created("crash-window".into(), AGENT.into(), request, now);
            journal
                .insert_command(&receipt, "hash", &json!({}))
                .unwrap();
            if stop_at != Created {
                for state in [Validating, Approved, Dispatched, Accepted, Applied] {
                    let state = if state == Applied && stop_at == TimedOut {
                        TimedOut
                    } else {
                        state
                    };
                    let state = if state == Applied && stop_at == UnknownOutcome {
                        UnknownOutcome
                    } else {
                        state
                    };
                    journal
                        .transition("crash-window", state, now, None, |r| {
                            if state == Accepted {
                                r.adapter_result = Some(AdapterResult {
                                    external_ref: Some("native:accepted-work".into()),
                                    ..AdapterResult::default()
                                });
                            }
                        })
                        .unwrap();
                    if state == stop_at {
                        break;
                    }
                }
            }
        }
        let engine = Engine::open(&path, clock(), allow_all()).unwrap();
        let (adapter, _, executions) = TrapAdapter::new();
        engine.register(Arc::new(adapter)).await.unwrap();
        engine.recover().await.unwrap();
        engine.recover().await.unwrap();
        let receipt = engine.receipt("crash-window").unwrap().unwrap();
        let expected = match stop_at {
            Created | Validating => Denied,
            Approved => Expired,
            Applied => Applied, // no stored expectation: remain pending, never infer success
            _ => UnknownOutcome,
        };
        assert_eq!(receipt.state, expected, "boundary {stop_at:?}");
        assert_eq!(executions.load(Ordering::SeqCst), 0, "boundary {stop_at:?}");
        if matches!(stop_at, Accepted | Applied | TimedOut | UnknownOutcome) {
            assert_eq!(
                receipt.adapter_result.unwrap().external_ref.as_deref(),
                Some("native:accepted-work")
            );
        }
        assert!(verify_evidence(&engine.journal().export_evidence().unwrap(), None).complete);
    }
}
