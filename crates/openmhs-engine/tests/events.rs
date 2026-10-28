//! The state fabric (ADR-0023): events arrive in order and correlated with the command
//! they were taken for; only callers allowed to `subscribe` see observations, and only of
//! capabilities that declare it; a consumer that falls behind is told what it missed; a
//! watch samples until it expires, is cancelled, or stops being allowed.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use openmhs_engine::{Engine, EngineError, EngineOptions, EventFilter, EventQuery, Journal};
use openmhs_policy::{Caller, Effect, Policy};
use openmhs_sim::{Faults, VirtualPsu};
use openmhs_types::descriptor::Operation;
use openmhs_types::event::{EventBody, EventKind, ObservationCause};
use openmhs_types::{Availability, CommandState};

const MEASURED: &str = "power.output.voltage.measured";

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

/// The usual grants plus `subscribe` on everything the supply streams.
fn with_subscribe() -> Policy {
    let mut p = allow_all();
    p.grants.push(grant(
        "grant.agent.subscribe",
        "*",
        &[Operation::Subscribe],
        Effect::Allow,
    ));
    p
}

async fn engine_with(policy: Policy, capacity: usize) -> (Engine, openmhs_sim::PsuHandle) {
    let clock = clock();
    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    let engine = Engine::with_journal(
        Journal::open_in_memory().unwrap(),
        clock,
        policy,
        EngineOptions {
            event_capacity: capacity,
            ..EngineOptions::default()
        },
    );
    engine.register(Arc::new(psu)).await.unwrap();
    (engine, handle)
}

/// `poll_events` with its query spelled out, as the tests use it.
#[allow(clippy::too_many_arguments)]
async fn poll(
    engine: &Engine,
    caller: &Caller,
    all: bool,
    stream: Option<&str>,
    after: Option<u64>,
    filter: &EventFilter,
    limit: usize,
    wait_ms: u64,
) -> openmhs_types::event::EventBatch {
    let query = EventQuery {
        stream: stream.map(str::to_owned),
        after,
        filter: filter.clone(),
        limit,
        wait_ms,
    };
    engine.poll_events(caller, all, &query).await
}

fn everything() -> EventFilter {
    EventFilter::default()
}

#[tokio::test(start_paused = true)]
async fn a_command_arrives_in_order_and_only_streamable_readings_are_delivered() {
    let (engine, _psu) = engine_with(with_subscribe(), 1_000).await;
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Verified);
    let b = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(0),
        &everything(),
        500,
        0,
    )
    .await;
    assert_eq!(b.missed, 0);
    let seqs: Vec<u64> = b.events.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");

    let states: Vec<CommandState> = b
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Command {
                command_id, state, ..
            } if *command_id == r.command_id => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states.first(),
        Some(&CommandState::Validating),
        "{states:?}"
    );
    assert_eq!(states.last(), Some(&CommandState::Verified), "{states:?}");
    assert!(states.contains(&CommandState::Dispatched));

    // The setpoint readback is not streamable (its descriptor does not declare subscribe),
    // so no observation of it is delivered, even to a caller allowed to subscribe.
    assert!(!b.events.iter().any(|e| matches!(&e.body,
        EventBody::Observation { capability, .. } if capability == SETPOINT)));

    // A streamable capability read for the command carries the command's id.
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    let b = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(b.next),
        &everything(),
        500,
        0,
    )
    .await;
    let obs: Vec<_> = b
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Observation {
                capability,
                cause,
                state,
                ..
            } if capability == MEASURED => Some((*cause, state.quality)),
            _ => None,
        })
        .collect();
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].0, ObservationCause::Read);
}

#[tokio::test(start_paused = true)]
async fn observations_need_a_subscribe_grant_and_commands_go_to_their_principal() {
    let (engine, _psu) = engine_with(allow_all(), 1_000).await;
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();

    // `read` is granted, `subscribe` is not: the agent sees its commands, not the stream.
    let b = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(0),
        &everything(),
        500,
        0,
    )
    .await;
    assert!(!b.events.is_empty());
    assert!(
        b.events.iter().all(|e| e.body.kind() == EventKind::Command),
        "{:?}",
        b.events
    );

    // Another principal sees none of them, and a credential scoped elsewhere none either.
    let other = Caller::in_process("agent.other");
    let b = poll(&engine, &other, false, None, Some(0), &everything(), 500, 0).await;
    assert!(b.events.is_empty(), "{:?}", b.events);
    // The operator view shows every command.
    let b = poll(&engine, &other, true, None, Some(0), &everything(), 500, 0).await;
    assert!(b.events.iter().any(|e| e.body.kind() == EventKind::Command));
}

#[tokio::test(start_paused = true)]
async fn a_consumer_that_falls_behind_is_told_how_many_it_missed() {
    let (engine, _psu) = engine_with(with_subscribe(), 4).await;
    for _ in 0..10 {
        engine.read(&agent(), PSU, MEASURED).await.unwrap();
    }
    let b = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(0),
        &everything(),
        500,
        0,
    )
    .await;
    assert_eq!(b.events.len(), 4, "only the buffer's worth is left");
    assert_eq!(
        b.missed, 6,
        "the rest is reported lost, not silently skipped"
    );
    assert_eq!(b.next, 10);

    // A small page continues where it left off, without losing or repeating anything.
    let first = poll(&engine, &agent(), false, None, Some(6), &everything(), 2, 0).await;
    let rest = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(first.next),
        &everything(),
        2,
        0,
    )
    .await;
    let seqs: Vec<u64> = first
        .events
        .iter()
        .chain(&rest.events)
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, [7, 8, 9, 10]);
    assert_eq!(first.missed + rest.missed, 0);
}

#[tokio::test(start_paused = true)]
async fn a_poll_waits_for_the_next_event_and_a_restart_resets_the_cursor() {
    let (engine, _psu) = engine_with(with_subscribe(), 100).await;
    let start = poll(&engine, &agent(), false, None, None, &everything(), 10, 0).await;
    assert!(start.events.is_empty(), "no cursor: only what happens next");

    let waiting = {
        let engine = engine.clone();
        let stream = start.stream.clone();
        tokio::spawn(async move {
            poll(
                &engine,
                &agent(),
                false,
                Some(&stream),
                Some(start.next),
                &EventFilter {
                    devices: vec!["sim.*".into()],
                    kinds: vec![EventKind::Observation],
                },
                10,
                20_000,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    engine.read(&agent(), PSU, MEASURED).await.unwrap();
    let got = waiting.await.unwrap();
    assert_eq!(got.events.len(), 1, "woken by the read, not by the timeout");

    let stale = poll(
        &engine,
        &agent(),
        false,
        Some("another-process"),
        Some(3),
        &everything(),
        10,
        0,
    )
    .await;
    assert!(stale.reset);
}

#[tokio::test(start_paused = true)]
async fn availability_changes_are_streamed() {
    let (engine, psu) = engine_with(with_subscribe(), 100).await;
    psu.set_faults(Faults {
        disconnected: true,
        ..Faults::default()
    });
    engine.refresh_availability(0).await.unwrap();
    let b = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(0),
        &everything(),
        100,
        0,
    )
    .await;
    assert!(b.events.iter().any(|e| matches!(
        &e.body,
        EventBody::Availability {
            to: Availability::Offline,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn a_watch_samples_until_it_expires_and_both_ends_are_audited() {
    let (engine, _psu) = engine_with(with_subscribe(), 1_000).await;
    let w = engine
        .create_watch(&agent(), PSU, MEASURED, 1_000, Some(5_500))
        .unwrap();
    assert_eq!(engine.watches(&agent(), false).len(), 1);
    tokio::time::sleep(Duration::from_millis(8_000)).await;

    let b = poll(
        &engine,
        &agent(),
        false,
        None,
        Some(0),
        &everything(),
        500,
        0,
    )
    .await;
    let samples = b
        .events
        .iter()
        .filter(|e| {
            matches!(
                &e.body,
                EventBody::Observation {
                    cause: ObservationCause::Watch,
                    ..
                }
            )
        })
        .count();
    assert_eq!(samples, 5, "one a second for 5.5 s");
    assert!(engine.watches(&agent(), false).is_empty(), "expired");

    let audit: Vec<_> = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind.starts_with("watch."))
        .collect();
    assert_eq!(
        audit.len(),
        2,
        "created and ended; samples are not audited one by one"
    );
    assert!(audit[1].data.contains("expired") && audit[1].data.contains(&w.id));
    assert!(engine.verify_audit().unwrap().is_valid());
}

#[tokio::test(start_paused = true)]
async fn a_watch_needs_subscribe_and_a_streamable_capability_and_stops_when_revoked() {
    // No subscribe grant.
    let (engine, _psu) = engine_with(allow_all(), 100).await;
    let e = engine
        .create_watch(&agent(), PSU, MEASURED, 1_000, None)
        .unwrap_err();
    assert!(
        matches!(&e, EngineError::Denied(o) if o.code == "policy.denied.no_grant"),
        "{e:?}"
    );

    let (engine, _psu) = engine_with(with_subscribe(), 100).await;
    // The setpoint does not declare subscribe.
    let e = engine
        .create_watch(&agent(), PSU, SETPOINT, 1_000, None)
        .unwrap_err();
    assert!(
        matches!(&e, EngineError::Denied(o) if o.code == "command.operation.unsupported"),
        "{e:?}"
    );
    // Too fast, or too long.
    for (every, ttl) in [(10, None), (1_000, Some(7_200_000))] {
        let e = engine
            .create_watch(&agent(), PSU, MEASURED, every, ttl)
            .unwrap_err();
        assert!(matches!(&e, EngineError::InvalidRequest(_)), "{e:?}");
    }

    // Revoking the grant ends the watch at its next sample.
    engine
        .create_watch(&agent(), PSU, MEASURED, 1_000, None)
        .unwrap();
    engine.set_policy(allow_all()).unwrap();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(engine.watches(&agent(), false).is_empty());
    let ended = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == "watch.ended")
        .unwrap();
    assert!(ended.data.contains("no longer allowed"), "{}", ended.data);
}

#[tokio::test(start_paused = true)]
async fn watches_are_bounded_replaced_and_cancelled_by_their_owner() {
    let (engine, _psu) = engine_with(with_subscribe(), 100).await;
    let first = engine
        .create_watch(&agent(), PSU, MEASURED, 1_000, None)
        .unwrap();
    let second = engine
        .create_watch(&agent(), PSU, MEASURED, 500, None)
        .unwrap();
    let mine = engine.watches(&agent(), false);
    assert_eq!(
        mine.len(),
        1,
        "the same capability again replaces the watch"
    );
    assert_eq!(mine[0].id, second.id);
    assert_ne!(first.id, second.id);

    let other = Caller::in_process("agent.other");
    assert!(engine.cancel_watch(&other, &second.id, false).is_err());
    engine.cancel_watch(&agent(), &second.id, false).unwrap();
    assert!(engine.watches(&agent(), true).is_empty());
}
