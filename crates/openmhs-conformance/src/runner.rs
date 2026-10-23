//! Runs the requirements against one adapter, through a real engine.
//!
//! L0 and L1 call the adapter directly, so the report can say exactly what it returned.
//! L2 goes through [`Engine`]: lifecycle states are the engine's, and the point of L2 is
//! that the adapter's answers make the engine reach the right ones.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use openmhs_adapter::{AdapterErrorKind, HardwareAdapter, Health};
use openmhs_engine::Engine;
use openmhs_policy::{Effect, Grant, Policy};
use openmhs_types::descriptor::{
    Capability, CapabilityKind, CompareOp, ConformanceLevel, Operation, PreconditionValue,
    SideEffect, Verification,
};
use openmhs_types::receipt::Decision;
use openmhs_types::state::MAX_FUTURE_SKEW_MS;
use openmhs_types::{
    Clock, CommandState, Descriptor, Quality, Receipt, Source, StateValue, SystemClock, Value,
};
use serde::Serialize;
use serde_json::json;
use tokio::time::Instant;

use crate::fixture::{Fault, Fixture};
use crate::report::{
    CheckResult, FixtureInfo, REPORT_VERSION, Report, Status, Subject, achieved_level, summarize,
};
use crate::requirements::requirement;
use crate::values;

/// The principal the suite acts as. Its policy allows everything, so only the device and
/// the descriptor decide what happens.
pub const PRINCIPAL: &str = "conformance:suite";

/// How a run behaves.
#[derive(Clone)]
pub struct Options {
    /// The clock the adapter timestamps readings with. With a simulator, pass the clock it
    /// was built with.
    pub clock: Arc<dyn Clock>,
    /// Bound for each adapter call the suite makes directly (describe, read, health …).
    pub call_timeout_ms: u64,
    /// Deadline for commands sent while a `no_response` or `lost_ack` fault is active.
    /// Non-idempotent capabilities get at least their `timing.timeout_ms`, so an adapter
    /// that would retry has time to do it.
    pub fault_deadline_ms: u64,
    /// Exercise capabilities whose side effect is `hazardous`. On by default because the
    /// suite is meant to run against fixtures and simulators. Their checks are `untested`
    /// when it is off.
    pub hazardous: bool,
    /// Values to use instead of generated ones: a value for a property, an object of
    /// parameters for an action. Keyed by capability name.
    pub samples: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Options {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            call_timeout_ms: 5_000,
            fault_deadline_ms: 1_000,
            hazardous: true,
            samples: Default::default(),
        }
    }

    /// Use `value` for every command the suite sends to `capability`.
    #[must_use]
    pub fn sample(mut self, capability: &str, value: serde_json::Value) -> Self {
        self.samples.insert(capability.to_owned(), value);
        self
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock))
    }
}

struct Finding {
    status: Status,
    message: String,
    evidence: Option<serde_json::Value>,
}

impl Finding {
    fn new(status: Status, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            evidence: None,
        }
    }
    fn pass(m: impl Into<String>) -> Self {
        Self::new(Status::Pass, m)
    }
    fn fail(m: impl Into<String>) -> Self {
        Self::new(Status::Fail, m)
    }
    fn untested(m: impl Into<String>) -> Self {
        Self::new(Status::Untested, m)
    }
    fn na(m: impl Into<String>) -> Self {
        Self::new(Status::NotApplicable, m)
    }
    fn with(mut self, evidence: serde_json::Value) -> Self {
        self.evidence = Some(evidence);
        self
    }
}

/// Why the suite could not send a command.
enum Problem {
    /// The device could not be brought into a state where the command is allowed.
    Setup(String),
    /// The engine refused a command built from the descriptor.
    Refused(String, serde_json::Value),
}

impl Problem {
    fn message(&self) -> &str {
        match self {
            Problem::Setup(m) | Problem::Refused(m, _) => m,
        }
    }
}

const PER_CAPABILITY_L2: [&str; 7] = [
    "L2.command",
    "L2.verification",
    "L2.reject",
    "L2.malformed",
    "L2.timeout",
    "L2.lost_ack",
    "L2.no_retry",
];

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The wire name of a serializable enum, e.g. `UNKNOWN_OUTCOME`.
fn wire<T: Serialize>(t: &T) -> String {
    match serde_json::to_value(t) {
        Ok(serde_json::Value::String(s)) => s,
        other => format!("{other:?}"),
    }
}

fn show(v: &Value) -> String {
    match v {
        Value::Quantity(q) => format!("{} {}", q.value, q.unit),
        Value::Integer(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Text(t) => format!("'{t}'"),
    }
}

fn applied(state: CommandState) -> bool {
    matches!(
        state,
        CommandState::Applied
            | CommandState::Verified
            | CommandState::VerificationFailed
            | CommandState::Unverifiable
    )
}

fn error_code(r: &Receipt) -> Option<&str> {
    r.adapter_result
        .as_ref()
        .and_then(|a| a.error.as_ref())
        .map(|e| e.code.as_str())
}

/// Every reason on a receipt, as `code: message`.
fn reasons(r: &Receipt) -> String {
    let all: Vec<String> = r
        .policy
        .errors
        .iter()
        .chain(r.adapter_result.as_ref().and_then(|a| a.error.as_ref()))
        .map(|e| format!("{}: {}", e.code, e.message))
        .collect();
    if all.is_empty() {
        String::new()
    } else {
        format!(" ({})", all.join("; "))
    }
}

fn evidence(r: &Receipt) -> serde_json::Value {
    json!({
        "command_id": r.command_id,
        "request": r.request,
        "state": r.state,
        "outcome": r.outcome,
        "policy_errors": r.policy.errors,
        "adapter_result": r.adapter_result,
        "verification": r.verification,
    })
}

fn at(r: &Receipt, state: CommandState) -> Option<openmhs_types::Timestamp> {
    r.transitions
        .iter()
        .find(|t| t.state == state)
        .map(|t| t.at)
}

fn is_state_changing(c: &Capability) -> bool {
    match c.kind {
        CapabilityKind::Property => c.supports(Operation::Write),
        CapabilityKind::Action => c.supports(Operation::Invoke),
        CapabilityKind::Event => false,
    }
}

fn policy() -> Policy {
    Policy {
        grants: vec![Grant {
            id: "conformance".into(),
            principal: PRINCIPAL.into(),
            device: "*".into(),
            capability: "*".into(),
            operations: vec![Operation::Read, Operation::Write, Operation::Invoke],
            effect: Effect::Allow,
            max_side_effect: None,
            reason: Some("conformance suite, against a fixture".into()),
        }],
        approvers: vec![],
    }
}

pub(crate) struct Runner {
    adapter: Arc<dyn HardwareAdapter>,
    fixture: Arc<dyn Fixture>,
    options: Options,
    engine: Option<Engine>,
    descriptor: Option<Descriptor>,
    descriptor_sha256: Option<String>,
    checks: Vec<CheckResult>,
    sources: Vec<Source>,
    /// When the suite last sent a command to each capability, for pacing.
    last: HashMap<String, Instant>,
    /// Whether any L2 fault was injected (recovery needs one).
    faulted: bool,
}

impl Runner {
    pub(crate) fn new(
        adapter: Arc<dyn HardwareAdapter>,
        fixture: Arc<dyn Fixture>,
        options: Options,
    ) -> Self {
        Self {
            adapter,
            fixture,
            options,
            engine: None,
            descriptor: None,
            descriptor_sha256: None,
            checks: Vec::new(),
            sources: Vec::new(),
            last: HashMap::new(),
            faulted: false,
        }
    }

    pub(crate) async fn run(mut self) -> Report {
        let started_at = self.options.clock.now();
        let t0 = Instant::now();
        if self.discover().await {
            self.observe().await;
            self.control().await;
        }
        self.record(
            "L3.leases",
            None,
            Instant::now(),
            Finding::untested(
                "L3 is defined with multi-device plans in M9; no adapter can claim it yet",
            ),
        );
        self.shutdown().await;
        self.finish(started_at, t0)
    }

    fn record(&mut self, id: &'static str, capability: Option<&str>, started: Instant, f: Finding) {
        debug_assert!(requirement(id).is_some(), "unregistered requirement {id}");
        let level = requirement(id).map_or(ConformanceLevel::L3, |r| r.level);
        self.checks.push(CheckResult {
            id: id.to_owned(),
            level,
            capability: capability.map(str::to_owned),
            status: f.status,
            message: f.message,
            evidence: f.evidence,
            duration_ms: ms(started.elapsed()),
        });
    }

    async fn bounded<T>(&self, fut: impl Future<Output = T>) -> Option<T> {
        tokio::time::timeout(Duration::from_millis(self.options.call_timeout_ms), fut)
            .await
            .ok()
    }

    fn no_answer(&self) -> String {
        format!("no answer within {} ms", self.options.call_timeout_ms)
    }

    fn note_source(&mut self, s: Source) {
        if !self.sources.contains(&s) {
            self.sources.push(s);
        }
    }

    fn engine(&self) -> &Engine {
        #[allow(clippy::expect_used)]
        self.engine
            .as_ref()
            .expect("engine exists once discovery succeeded")
    }

    fn device_id(&self) -> String {
        self.descriptor
            .as_ref()
            .map(|d| d.device.id.clone())
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------- L0

    /// L0 up to registration. Returns whether the device is registered and usable.
    async fn discover(&mut self) -> bool {
        let t = Instant::now();
        let json = match self.bounded(self.adapter.describe()).await {
            None => {
                let m = self.no_answer();
                self.record("L0.describe", None, t, Finding::fail(m));
                return false;
            }
            Some(Err(e)) => {
                self.record(
                    "L0.describe",
                    None,
                    t,
                    Finding::fail(format!("describe failed: {e}")),
                );
                return false;
            }
            Some(Ok(json)) => json,
        };
        self.descriptor_sha256 = serde_json::to_string(&json)
            .ok()
            .map(|s| openmhs_engine::journal::sha256_hex(&s));
        self.record(
            "L0.describe",
            None,
            t,
            Finding::pass("returned a descriptor"),
        );

        let t = Instant::now();
        let descriptor = match openmhs_types::validate_descriptor(&json) {
            Ok(d) => d,
            Err(issues) => {
                let first = issues
                    .first()
                    .map(|i| format!("; first: [{}] {} at '{}'", i.code, i.message, i.pointer))
                    .unwrap_or_default();
                self.record(
                    "L0.descriptor",
                    None,
                    t,
                    Finding::fail(format!("{} problem(s){first}", issues.len()))
                        .with(json!({ "issues": issues })),
                );
                return false;
            }
        };
        self.record(
            "L0.descriptor",
            None,
            t,
            Finding::pass(format!(
                "valid: device '{}', {} capabilities",
                descriptor.device.id,
                descriptor.capabilities.len()
            )),
        );
        self.descriptor = Some(descriptor.clone());

        let t = Instant::now();
        let id = self.adapter.adapter_id().to_owned();
        if descriptor.adapter.id == id {
            self.record("L0.adapter_id", None, t, Finding::pass(format!("'{id}'")));
        } else {
            self.record(
                "L0.adapter_id",
                None,
                t,
                Finding::fail(format!(
                    "adapter_id() returns '{id}' but the descriptor names '{}'; the engine will not register a mismatched adapter",
                    descriptor.adapter.id
                )),
            );
            return false;
        }

        let t = Instant::now();
        let f = match self.bounded(self.adapter.describe()).await {
            Some(Ok(again)) if again == json => Finding::pass("same descriptor both times"),
            Some(Ok(_)) => Finding::fail(
                "a second describe returned a different descriptor; it must not change while the adapter is running",
            ),
            Some(Err(e)) => Finding::fail(format!("the second describe failed: {e}")),
            None => Finding::fail(format!("the second describe: {}", self.no_answer())),
        };
        self.record("L0.describe_stable", None, t, f);

        let t = Instant::now();
        let engine = match Engine::in_memory(self.options.clock.clone(), policy()) {
            Ok(e) => e,
            Err(e) => {
                self.record(
                    "L0.connect",
                    None,
                    t,
                    Finding::untested(format!("the suite could not start an engine: {e}")),
                );
                return false;
            }
        };
        match self.bounded(engine.register(self.adapter.clone())).await {
            Some(Ok(_)) => {
                self.record(
                    "L0.connect",
                    None,
                    t,
                    Finding::pass("connected and registered"),
                );
            }
            Some(Err(e)) => {
                self.record(
                    "L0.connect",
                    None,
                    t,
                    Finding::fail(format!("registration failed: [{}] {e}", e.code())),
                );
                return false;
            }
            None => {
                let m = self.no_answer();
                self.record("L0.connect", None, t, Finding::fail(m));
                return false;
            }
        }
        self.engine = Some(engine);

        let t = Instant::now();
        let f = match self.bounded(self.adapter.health()).await {
            Some(Health::Healthy) => Finding::pass("healthy"),
            Some(Health::Degraded(m)) => Finding::fail(format!("reports degraded: {m}")),
            Some(Health::Disconnected(m)) => Finding::fail(format!("reports disconnected: {m}")),
            None => Finding::fail(self.no_answer()),
        };
        self.record("L0.health", None, t, f);
        true
    }

    async fn shutdown(&mut self) {
        let t = Instant::now();
        let f = match self.bounded(self.adapter.shutdown()).await {
            Some(Ok(())) => Finding::pass("shut down cleanly"),
            Some(Err(e)) => Finding::fail(format!("shutdown failed: {e}")),
            None => Finding::fail(self.no_answer()),
        };
        self.record("L0.shutdown", None, t, f);
    }

    // ------------------------------------------------------------------- L1

    fn capabilities(&self, keep: impl Fn(&Capability) -> bool) -> Vec<Capability> {
        self.descriptor
            .iter()
            .flat_map(|d| d.capabilities.iter())
            .filter(|c| keep(c))
            .cloned()
            .collect()
    }

    async fn observe(&mut self) {
        let mut readable = Vec::new();
        for cap in self.capabilities(Capability::is_readable_property) {
            if self.read_checks(&cap).await {
                readable.push(cap);
            }
        }
        self.record(
            "L1.subscribe",
            None,
            Instant::now(),
            Finding::na(
                "deferred: the adapter contract (v0.1) has no subscription API, so there is nothing to test yet (ADR-0013)",
            ),
        );
        self.disconnect_checks(&readable).await;
    }

    /// Returns whether the property could be read at all.
    async fn read_checks(&mut self, cap: &Capability) -> bool {
        let name = cap.name.as_str();
        let t = Instant::now();
        let sv = match self.bounded(self.adapter.read(name)).await {
            Some(Ok(sv)) => sv,
            other => {
                let m = match other {
                    Some(Err(e)) => format!("read failed: {e}"),
                    _ => self.no_answer(),
                };
                self.record("L1.read", Some(name), t, Finding::fail(m));
                for id in ["L1.read.value", "L1.read.timestamp", "L1.read.quality"] {
                    self.record(
                        id,
                        Some(name),
                        t,
                        Finding::untested("needs a reading; see L1.read"),
                    );
                }
                return false;
            }
        };
        self.note_source(sv.source);
        let ev = json!({ "reading": sv });
        self.record(
            "L1.read",
            Some(name),
            t,
            Finding::pass(format!("read {}", show(&sv.value))).with(ev.clone()),
        );

        let t = Instant::now();
        let f = match cap.value.as_ref() {
            Some(spec) => match values::check_reading(spec, &sv.value) {
                Ok(()) => Finding::pass(format!("{} matches the declaration", show(&sv.value))),
                Err(m) => Finding::fail(m).with(ev.clone()),
            },
            None => Finding::fail("the property declares no value"),
        };
        self.record("L1.read.value", Some(name), t, f);

        let t = Instant::now();
        let age = sv.age_ms(self.options.clock.now());
        let f = if age < -MAX_FUTURE_SKEW_MS {
            Finding::fail(format!(
                "timestamped {} ms in the future; use the time the value was measured",
                -age
            ))
        } else if let Some(max) = cap.freshness.map(|f| f.max_age_ms)
            && age > i64::try_from(max).unwrap_or(i64::MAX)
        {
            Finding::fail(format!(
                "{age} ms old when returned, but the descriptor declares freshness.max_age_ms = {max}"
            ))
        } else {
            Finding::pass(format!("{} ms old", age.max(0)))
        };
        self.record("L1.read.timestamp", Some(name), t, f.with(ev.clone()));

        let t = Instant::now();
        let f = if sv.quality == Quality::Good {
            Finding::pass("GOOD")
        } else {
            Finding::fail(format!(
                "{} although the device is healthy",
                wire(&sv.quality)
            ))
        };
        self.record("L1.read.quality", Some(name), t, f);
        true
    }

    /// `readable`: the properties that read before the outage, so that what is judged
    /// is the outage and not a property that never worked.
    async fn disconnect_checks(&mut self, readable: &[Capability]) {
        const IDS: [&str; 3] = ["L1.disconnect.read", "L1.disconnect.health", "L1.reconnect"];
        let t = Instant::now();
        if readable.is_empty()
            && !self
                .capabilities(Capability::is_readable_property)
                .is_empty()
        {
            for id in IDS {
                self.record(
                    id,
                    None,
                    t,
                    Finding::untested("no property could be read; see L1.read"),
                );
            }
            return;
        }
        if !self.fixture.supports(Fault::LinkDown) {
            for id in IDS {
                self.record(
                    id,
                    None,
                    t,
                    Finding::untested("the fixture cannot inject link_down"),
                );
            }
            return;
        }
        if let Err(e) = self.fixture.inject(Fault::LinkDown).await {
            let _ = self.fixture.clear().await;
            for id in IDS {
                self.record(
                    id,
                    None,
                    t,
                    Finding::untested(format!("the fixture failed to inject link_down: {e}")),
                );
            }
            return;
        }

        for cap in readable {
            let t = Instant::now();
            let f = match self.bounded(self.adapter.read(&cap.name)).await {
                None => Finding::pass(format!(
                    "the read hung until the suite gave up after {} ms; the engine bounds reads and reports adapter.timeout, but failing fast is better",
                    self.options.call_timeout_ms
                )),
                Some(Err(e))
                    if matches!(
                        e.kind,
                        AdapterErrorKind::Disconnected
                            | AdapterErrorKind::Timeout
                            | AdapterErrorKind::NotSent
                    ) =>
                {
                    Finding::pass(format!("failed with {}", e.kind.code()))
                }
                Some(Err(e)) => Finding::fail(format!(
                    "failed with {}; a lost link is adapter.disconnected (or adapter.timeout)",
                    e.kind.code()
                )),
                Some(Ok(sv)) if sv.quality != Quality::Good => {
                    Finding::pass(format!("returned a {} reading", wire(&sv.quality)))
                }
                Some(Ok(sv)) => {
                    let age = sv.age_ms(self.options.clock.now());
                    Finding::fail(format!(
                        "returned a GOOD reading ({}, {age} ms old) while the link was down; mark cached values BAD, or fail with adapter.disconnected",
                        show(&sv.value)
                    ))
                    .with(json!({ "reading": sv }))
                }
            };
            self.record("L1.disconnect.read", Some(&cap.name), t, f);
        }

        let t = Instant::now();
        let f = match self.bounded(self.adapter.health()).await {
            Some(Health::Healthy) => Finding::fail("reports healthy while the link is down"),
            Some(Health::Degraded(m)) => Finding::pass(format!("degraded: {m}")),
            Some(Health::Disconnected(m)) => Finding::pass(format!("disconnected: {m}")),
            None => Finding::pass(format!(
                "health did not answer within {} ms",
                self.options.call_timeout_ms
            )),
        };
        self.record("L1.disconnect.health", None, t, f);

        let t = Instant::now();
        if let Err(e) = self.fixture.clear().await {
            self.record(
                "L1.reconnect",
                None,
                t,
                Finding::untested(format!("the fixture could not restore the link: {e}")),
            );
            return;
        }
        let f = self.await_recovery(readable).await;
        self.record("L1.reconnect", None, t, f);
    }

    /// Poll until every readable property reads `GOOD` and health is healthy again.
    async fn await_recovery(&self, readable: &[Capability]) -> Finding {
        let start = Instant::now();
        let limit = Duration::from_millis(self.options.call_timeout_ms);
        loop {
            let mut problems = Vec::new();
            for cap in readable {
                match self.bounded(self.adapter.read(&cap.name)).await {
                    Some(Ok(sv)) if sv.quality == Quality::Good => {}
                    Some(Ok(sv)) => {
                        problems.push(format!("{} reads {}", cap.name, wire(&sv.quality)));
                    }
                    Some(Err(e)) => problems.push(format!("{}: {e}", cap.name)),
                    None => problems.push(format!("{}: {}", cap.name, self.no_answer())),
                }
            }
            match self.bounded(self.adapter.health()).await {
                Some(Health::Healthy) => {}
                Some(h) => problems.push(format!("health is {h:?}")),
                None => problems.push(format!("health: {}", self.no_answer())),
            }
            if problems.is_empty() {
                return Finding::pass(format!("back after {} ms", ms(start.elapsed())));
            }
            if start.elapsed() >= limit {
                return Finding::fail(format!(
                    "{} ms after the link returned: {}",
                    ms(start.elapsed()),
                    problems.join("; ")
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // ------------------------------------------------------------------- L2

    async fn control(&mut self) {
        let caps = self.capabilities(is_state_changing);
        if caps.is_empty() {
            let t = Instant::now();
            for id in ["L2.command", "L2.recovery"] {
                self.record(
                    id,
                    None,
                    t,
                    Finding::na("the descriptor declares no writable property or action"),
                );
            }
            return;
        }
        let mut working = None;
        for cap in &caps {
            if self.exercise(cap).await && working.is_none() {
                working = Some(cap.clone());
            }
        }

        let t = Instant::now();
        let f = match (self.faulted, working) {
            (false, _) => Finding::untested(
                "no fault was injected (the fixture supports none of refuse, garbled, no_response, lost_ack), so there was nothing to recover from",
            ),
            (true, None) => {
                Finding::untested("no command was carried out, so recovery cannot be shown")
            }
            (true, Some(cap)) => match self.attempt(&cap).await {
                Ok(r) if applied(r.state) => Finding::pass(format!(
                    "'{}' carried out after the faults cleared",
                    cap.name
                )),
                Ok(r) => Finding::fail(format!(
                    "after the faults cleared, '{}' ended {}{}",
                    cap.name,
                    wire(&r.state),
                    reasons(&r)
                ))
                .with(evidence(&r)),
                Err(p) => Finding::fail(format!(
                    "after the faults cleared, '{}' could not be sent: {}",
                    cap.name,
                    p.message()
                )),
            },
        };
        self.record("L2.recovery", None, t, f);
    }

    /// Run every per-capability L2 check on `cap`. Returns whether a valid command was
    /// carried out.
    async fn exercise(&mut self, cap: &Capability) -> bool {
        let name = cap.name.as_str();
        let t = Instant::now();
        if cap.side_effect == SideEffect::Hazardous && !self.options.hazardous {
            for id in PER_CAPABILITY_L2 {
                self.record(
                    id,
                    Some(name),
                    t,
                    Finding::untested(
                        "hazardous capability not exercised (Options::hazardous is off)",
                    ),
                );
            }
            return false;
        }

        let baseline = match self.attempt(cap).await {
            Ok(r) => r,
            Err(p) => {
                let (first, rest) = match p {
                    Problem::Setup(m) => (Finding::untested(m.clone()), m),
                    Problem::Refused(m, ev) => (
                        Finding::fail(m).with(ev),
                        "needs a command the engine allows; see L2.command".to_owned(),
                    ),
                };
                self.record("L2.command", Some(name), t, first);
                for id in &PER_CAPABILITY_L2[1..] {
                    self.record(id, Some(name), t, Finding::untested(rest.clone()));
                }
                return false;
            }
        };
        let ok = applied(baseline.state);
        self.record("L2.command", Some(name), t, command_finding(cap, &baseline));
        self.record(
            "L2.verification",
            Some(name),
            Instant::now(),
            verification_finding(cap, &baseline),
        );
        if !ok {
            // A fault test judges how a working command fails; with no working command
            // its verdict would only repeat L2.command, and its advice would mislead.
            let t = Instant::now();
            for id in &PER_CAPABILITY_L2[2..] {
                self.record(
                    id,
                    Some(name),
                    t,
                    Finding::untested("needs a command that is carried out; see L2.command"),
                );
            }
            return false;
        }

        let mut deliveries: Vec<(Fault, u64)> = Vec::new();
        for (fault, id) in [
            (Fault::Refuse, "L2.reject"),
            (Fault::Garbled, "L2.malformed"),
            (Fault::NoResponse, "L2.timeout"),
            (Fault::LostAck, "L2.lost_ack"),
        ] {
            let t = Instant::now();
            if !self.fixture.supports(fault) {
                self.record(
                    id,
                    Some(name),
                    t,
                    Finding::untested(format!("the fixture cannot inject {fault}")),
                );
                continue;
            }
            let f = self.under_fault(cap, fault, &mut deliveries).await;
            self.record(id, Some(name), t, f);
        }

        let t = Instant::now();
        let f = if cap.idempotent == Some(true) {
            Finding::na("declared idempotent: repeating it is harmless, so retries are allowed")
        } else if self.fixture.deliveries().is_none() {
            Finding::untested("the fixture cannot count deliveries")
        } else if let Some((fault, n)) = deliveries.iter().find(|(_, n)| *n > 1) {
            Finding::fail(format!(
                "under {fault} one command reached the device {n} times: the adapter retried a non-idempotent command. Return the error instead; the engine decides what happens next"
            ))
            .with(json!({ "deliveries": deliveries }))
        } else if !deliveries.iter().any(|(f, _)| *f == Fault::NoResponse) {
            Finding::untested("needs a no_response check, which did not run")
        } else {
            let seen: Vec<String> = deliveries
                .iter()
                .map(|(f, n)| format!("{f}: {n}"))
                .collect();
            Finding::pass(format!(
                "each command reached the device at most once ({})",
                seen.join(", ")
            ))
        };
        self.record("L2.no_retry", Some(name), t, f);
        ok
    }

    /// Send one valid command to `cap` while `fault` is active, and judge the result.
    async fn under_fault(
        &mut self,
        cap: &Capability,
        fault: Fault,
        deliveries: &mut Vec<(Fault, u64)>,
    ) -> Finding {
        let request = match self.prepare(cap, None, 0).await {
            Ok(r) => r,
            Err(p) => return Finding::untested(p.message().to_owned()),
        };
        // What the readback said before, to tell whether reconciliation can prove anything.
        let prior = match (fault, values::readback_capability(cap)) {
            (Fault::LostAck, Some(rb)) => self.current(rb).await,
            _ => None,
        };
        let before = self.fixture.deliveries();
        if let Err(e) = self.fixture.inject(fault).await {
            let _ = self.fixture.clear().await;
            return Finding::untested(format!("the fixture failed to inject {fault}: {e}"));
        }
        self.faulted = true;
        let deadline = match fault {
            Fault::NoResponse | Fault::LostAck => self.fault_deadline(cap),
            _ => deadline(cap),
        };
        let submitted = self.submit(cap, &request, deadline).await;
        let cleared = self.fixture.clear().await;
        if let (Some(b), Some(a)) = (before, self.fixture.deliveries()) {
            deliveries.push((fault, a.saturating_sub(b)));
        }
        if let Err(e) = cleared {
            return Finding::untested(format!("the fixture could not clear {fault}: {e}"));
        }
        let receipt = match submitted {
            Ok(r) => r,
            Err(e) => return Finding::fail(format!("the engine refused the command: {e}")),
        };
        let state = receipt.state;
        let code = error_code(&receipt).unwrap_or("no error").to_owned();
        let s = wire(&state);
        let f = match fault {
            Fault::Refuse => match state {
                CommandState::Rejected if code == "adapter.rejected" => {
                    Finding::pass("REJECTED with adapter.rejected")
                }
                CommandState::Failed if code == "adapter.rejected" => Finding::fail(
                    "ended FAILED: the adapter reported ACCEPTED (progress.accepted()) before the device refused; report acceptance only once the device acknowledges",
                ),
                s if applied(s) => {
                    Finding::fail("the device refused, but the adapter reported it applied")
                }
                _ => Finding::fail(format!(
                    "ended {s} with {code}; a refusal is returned as ExecuteReport::Rejected"
                )),
            },
            Fault::Garbled => match state {
                CommandState::UnknownOutcome if code == "adapter.malformed_response" => {
                    Finding::pass("UNKNOWN_OUTCOME with adapter.malformed_response")
                }
                s if applied(s) => Finding::fail(
                    "the device answered with something unparseable, but the adapter reported success",
                ),
                CommandState::UnknownOutcome => Finding::fail(format!(
                    "ended UNKNOWN_OUTCOME with {code}; an answer that cannot be parsed is AdapterErrorKind::MalformedResponse"
                )),
                _ => Finding::fail(format!(
                    "ended {s} with {code}; the device may have applied it, so return AdapterErrorKind::MalformedResponse, which keeps the outcome uncertain"
                )),
            },
            Fault::NoResponse => match state {
                CommandState::UnknownOutcome => Finding::pass(format!("UNKNOWN_OUTCOME ({code})")),
                s if applied(s) => {
                    Finding::fail("no answer came, yet the adapter reported success")
                }
                _ => Finding::fail(format!(
                    "ended {s} with {code}; without an answer nothing is known, so return AdapterErrorKind::Timeout (or let the deadline pass), never NotSent or a failure"
                )),
            },
            Fault::LostAck => self.judge_lost_ack(cap, &receipt, prior.as_ref()).await,
            Fault::LinkDown => Finding::untested("link_down is an L1 fault"),
        };
        f.with(evidence(&receipt))
    }

    async fn judge_lost_ack(
        &mut self,
        cap: &Capability,
        receipt: &Receipt,
        prior: Option<&Value>,
    ) -> Finding {
        let s = wire(&receipt.state);
        if receipt.state != CommandState::UnknownOutcome {
            return if applied(receipt.state) {
                Finding::fail("the answer was lost, yet the adapter reported success")
            } else {
                Finding::fail(format!(
                    "ended {s}{}; a lost answer must leave the outcome uncertain",
                    reasons(receipt)
                ))
            };
        }
        let Some(readback) = values::readback_capability(cap).map(str::to_owned) else {
            return Finding::pass(
                "UNKNOWN_OUTCOME; no readback is declared, so an operator resolves it",
            );
        };
        let expected = self.await_readback(cap, &readback, receipt).await;
        let already = match (prior, &expected) {
            (Some(p), Some((e, tol))) => openmhs_policy::readback_matches(e, p, *tol),
            _ => false,
        };
        match self.engine().reconcile(&receipt.command_id).await {
            Ok(r) if r.state == CommandState::ReconciledApplied => Finding::pass(format!(
                "UNKNOWN_OUTCOME, then reconciled as applied by reading back '{readback}'{}",
                if already {
                    " (the readback already held before the command, so this shows the path, not that this command applied)"
                } else {
                    ""
                }
            )),
            Ok(r) => Finding::fail(format!(
                "reconciliation read '{readback}' and judged the command {}, although the fixture applied it: the readback does not reflect this command, or the device needs longer than the declared verification timeout",
                wire(&r.state)
            ))
            .with(evidence(&r)),
            Err(e) => Finding::fail(format!("reconcile failed: {e}")),
        }
    }

    /// Give a command that was applied without an answer time to finish, by polling its
    /// readback until it matches (bounded by the declared verification timeout). Returns
    /// the expected value and tolerance.
    async fn await_readback(
        &self,
        cap: &Capability,
        readback: &str,
        receipt: &Receipt,
    ) -> Option<(Value, Option<openmhs_types::descriptor::Tolerance>)> {
        let (expected, tolerance, timeout_ms) = match &cap.verification {
            Some(Verification::Readback {
                expect,
                expect_param,
                tolerance,
                timeout_ms,
                ..
            }) => {
                let expected = match (expect, expect_param) {
                    (Some(v), _) => Some(v.clone()),
                    (None, Some(p)) => receipt
                        .request
                        .params
                        .as_ref()
                        .and_then(|m| m.get(p))
                        .cloned(),
                    (None, None) => receipt.request.value.clone(),
                };
                (
                    expected,
                    *tolerance,
                    timeout_ms.unwrap_or(self.options.call_timeout_ms),
                )
            }
            _ => return None,
        };
        let expected = expected?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(timeout_ms) {
            if let Some(Ok(sv)) = self.bounded(self.adapter.read(readback)).await
                && openmhs_policy::readback_matches(&expected, &sv.value, tolerance)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Some((expected, tolerance))
    }

    fn fault_deadline(&self, cap: &Capability) -> u64 {
        let base = self.options.fault_deadline_ms;
        if cap.idempotent == Some(true) {
            base
        } else {
            base.max(cap.timing.map_or(base, |t| t.timeout_ms))
        }
    }

    /// Prepare and send one valid command to `cap`.
    async fn attempt(&mut self, cap: &Capability) -> Result<Receipt, Problem> {
        let request = self.prepare(cap, None, 0).await?;
        self.submit(cap, &request, deadline(cap))
            .await
            .map_err(|e| Problem::Refused(format!("the engine rejected the command: {e}"), request))
    }

    async fn submit(
        &mut self,
        cap: &Capability,
        request: &serde_json::Value,
        deadline_ms: u64,
    ) -> Result<Receipt, openmhs_engine::EngineError> {
        let mut req = request.clone();
        req["deadline_ms"] = json!(deadline_ms);
        self.last.insert(cap.name.clone(), Instant::now());
        self.engine().submit(PRINCIPAL, &req).await
    }

    /// Wait out the rate and slew windows since the last command to `cap`.
    async fn pace(&self, cap: &Capability) {
        let window = [cap.slew.map(|s| s.per_ms), cap.rate_limit.map(|r| r.per_ms)]
            .into_iter()
            .flatten()
            .max();
        if let (Some(w), Some(last)) = (window, self.last.get(&cap.name)) {
            let need = Duration::from_millis(w + 10);
            let elapsed = last.elapsed();
            if elapsed < need {
                tokio::time::sleep(need - elapsed).await;
            }
        }
    }

    async fn current(&self, capability: &str) -> Option<Value> {
        match self.bounded(self.adapter.read(capability)).await {
            Some(Ok(StateValue { value, .. })) => Some(value),
            _ => None,
        }
    }

    /// Build a request for `cap` from the descriptor and the device's current state.
    async fn build(
        &self,
        cap: &Capability,
        fixed: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, Problem> {
        let target = json!({ "device": self.device_id(), "capability": cap.name });
        let sample = self.options.samples.get(&cap.name).cloned();
        if cap.kind == CapabilityKind::Property {
            let value = match fixed.or(sample) {
                Some(v) => v,
                None => {
                    let current = self.current(&cap.name).await;
                    values::write_value(cap, current.as_ref()).map_err(Problem::Setup)?
                }
            };
            Ok(json!({ "target": target, "operation": "write", "value": value }))
        } else {
            let params = match sample {
                Some(serde_json::Value::Object(m)) => m,
                Some(_) => {
                    return Err(Problem::Setup(format!(
                        "the sample for action '{}' must be an object of parameters",
                        cap.name
                    )));
                }
                None => {
                    let current = match values::readback_capability(cap) {
                        Some(rb) => self.current(rb).await,
                        None => None,
                    };
                    values::params(cap, current.as_ref()).map_err(Problem::Setup)?
                }
            };
            Ok(json!({ "target": target, "operation": "invoke", "params": params }))
        }
    }

    /// Build a request the engine allows right now, establishing preconditions on the
    /// way when the descriptor says how (a writable property, or an action whose
    /// readback sets it). Checked with preflight, so nothing is sent until it would pass.
    async fn prepare(
        &mut self,
        cap: &Capability,
        fixed: Option<serde_json::Value>,
        depth: usize,
    ) -> Result<serde_json::Value, Problem> {
        for _ in 0..6 {
            let request = self.build(cap, fixed.clone()).await?;
            self.pace(cap).await;
            let pre = self
                .engine()
                .preflight(PRINCIPAL, &request)
                .await
                .map_err(|e| {
                    Problem::Refused(
                        format!("the engine rejected a request built from the descriptor: {e}"),
                        json!({ "request": request }),
                    )
                })?;
            match pre.decision {
                Decision::Allow => return Ok(request),
                Decision::RequireApproval => {
                    return Err(Problem::Setup(
                        "the suite's policy unexpectedly requires approval".into(),
                    ));
                }
                Decision::Deny => {}
            }
            let Some(e) = pre.errors.first() else {
                return Err(Problem::Setup("denied without a reason".into()));
            };
            match e.code.as_str() {
                "command.precondition.failed" => {
                    let rule = e.rule_id.clone().unwrap_or_default();
                    Box::pin(self.establish(cap, &rule, depth)).await?;
                }
                "command.rate_limited" | "command.slew_exceeded" => {
                    let w = [cap.slew.map(|s| s.per_ms), cap.rate_limit.map(|r| r.per_ms)]
                        .into_iter()
                        .flatten()
                        .max()
                        .unwrap_or(100);
                    tokio::time::sleep(Duration::from_millis(w + 10)).await;
                }
                code => {
                    return Err(Problem::Refused(
                        format!(
                            "the engine refused a command built from the descriptor: [{code}] {}",
                            e.message
                        ),
                        json!({ "request": request, "errors": pre.errors }),
                    ));
                }
            }
        }
        Err(Problem::Setup(format!(
            "could not get an allowed command for '{}' after several attempts",
            cap.name
        )))
    }

    /// Make precondition `rule` of `cap` hold, using what the descriptor declares.
    async fn establish(
        &mut self,
        cap: &Capability,
        rule: &str,
        depth: usize,
    ) -> Result<(), Problem> {
        if depth >= 3 {
            return Err(Problem::Setup(format!(
                "preconditions of '{}' nest too deeply to establish",
                cap.name
            )));
        }
        let Some(p) = cap.preconditions.iter().find(|p| p.id == rule).cloned() else {
            return Err(Problem::Setup(format!(
                "denied by precondition '{rule}', which '{}' does not declare",
                cap.name
            )));
        };
        let want = match (p.op, &p.value) {
            (CompareOp::Eq, PreconditionValue::One(v)) => v.clone(),
            (CompareOp::In, PreconditionValue::Many(vs)) if !vs.is_empty() => vs[0].clone(),
            _ => {
                return Err(Problem::Setup(format!(
                    "precondition '{}' ({} {}) cannot be established automatically; set the device up first, or pass a sample",
                    p.id,
                    p.capability,
                    wire(&p.op)
                )));
            }
        };
        let all = self.capabilities(|_| true);
        let property = all.iter().find(|c| {
            c.name == p.capability
                && c.kind == CapabilityKind::Property
                && c.supports(Operation::Write)
        });
        let action = all.iter().find(|c| {
            c.kind == CapabilityKind::Action
                && c.supports(Operation::Invoke)
                && matches!(&c.verification, Some(Verification::Readback { capability: Some(rb), expect: Some(v), .. })
                    if *rb == p.capability && *v == want)
        });
        let (remedy, fixed) = match (property, action) {
            (Some(c), _) => (c.clone(), Some(json!(want))),
            (None, Some(c)) => (c.clone(), None),
            (None, None) => {
                return Err(Problem::Setup(format!(
                    "precondition '{}' needs {} = {}, and no writable property or action declares that it sets it (an action with verification.readback of '{}' and that expect); set the device up first",
                    p.id,
                    p.capability,
                    show(&want),
                    p.capability
                )));
            }
        };
        if remedy.side_effect == SideEffect::Hazardous && !self.options.hazardous {
            return Err(Problem::Setup(format!(
                "precondition '{}' needs '{}', which is hazardous (Options::hazardous is off)",
                p.id, remedy.name
            )));
        }
        let request = Box::pin(self.prepare(&remedy, fixed, depth + 1)).await?;
        let receipt = self
            .submit(&remedy, &request, deadline(&remedy))
            .await
            .map_err(|e| Problem::Setup(format!("establishing '{}': {e}", p.id)))?;
        if applied(receipt.state) {
            Ok(())
        } else {
            Err(Problem::Setup(format!(
                "tried to establish precondition '{}' with '{}', which ended {}{}",
                p.id,
                remedy.name,
                wire(&receipt.state),
                reasons(&receipt)
            )))
        }
    }

    fn finish(mut self, started_at: openmhs_types::Timestamp, t0: Instant) -> Report {
        self.checks.sort_by_key(|c| c.level);
        let achieved = achieved_level(&self.checks);
        let declared = self.descriptor.as_ref().and_then(|d| d.adapter.conformance);
        let passed = match declared {
            Some(d) => achieved.is_some_and(|a| a >= d),
            None => achieved.is_some(),
        };
        let faults = Fault::ALL
            .into_iter()
            .filter(|f| self.fixture.supports(*f))
            .collect();
        Report {
            openmhs_conformance: REPORT_VERSION.into(),
            suite_version: env!("CARGO_PKG_VERSION").into(),
            subject: Subject {
                adapter: self.adapter.adapter_id().to_owned(),
                adapter_version: self.descriptor.as_ref().map(|d| d.adapter.version.clone()),
                device: self.descriptor.as_ref().map(|d| d.device.id.clone()),
                descriptor_sha256: self.descriptor_sha256.take(),
                sources: std::mem::take(&mut self.sources),
            },
            fixture: FixtureInfo {
                faults,
                counts_deliveries: self.fixture.deliveries().is_some(),
            },
            declared,
            achieved,
            passed,
            started_at,
            duration_ms: ms(t0.elapsed()),
            summary: summarize(&self.checks),
            checks: self.checks,
        }
    }
}

/// Deadline for a normal command: the declared timeout plus a margin.
fn deadline(cap: &Capability) -> u64 {
    cap.timing.map_or(10_000, |t| t.timeout_ms) + 1_000
}

fn command_finding(cap: &Capability, r: &Receipt) -> Finding {
    let s = wire(&r.state);
    if applied(r.state) {
        let took = match (
            at(r, CommandState::Dispatched),
            at(r, CommandState::Applied),
        ) {
            (Some(a), Some(b)) => format!(" in {} ms", a.ms_until(b).max(0)),
            _ => String::new(),
        };
        let declared = match cap.timing {
            Some(t) => match t.expected_ms {
                Some(e) => format!(" (expected about {e} ms, limit {} ms)", t.timeout_ms),
                None => format!(" (limit {} ms)", t.timeout_ms),
            },
            None => String::new(),
        };
        return Finding::pass(format!("carried out{took}{declared}")).with(evidence(r));
    }
    let hint = match r.state {
        CommandState::UnknownOutcome if error_code(r) == Some("adapter.timeout") => {
            "; the adapter took longer than timing.timeout_ms, or hung"
        }
        CommandState::Rejected => "; the device refused a command the descriptor says is valid",
        CommandState::Denied => "; the engine refused it",
        _ => "",
    };
    Finding::fail(format!("a valid command ended {s}{}{hint}", reasons(r))).with(evidence(r))
}

fn verification_finding(cap: &Capability, r: &Receipt) -> Finding {
    if !applied(r.state) {
        return Finding::untested("the command was not carried out; see L2.command");
    }
    let rb = values::readback_capability(cap).unwrap_or(&cap.name);
    let observed = r
        .verification
        .as_ref()
        .and_then(|v| v.observed.as_ref())
        .map_or_else(|| "nothing".to_owned(), |o| show(&o.value));
    let expected = r
        .verification
        .as_ref()
        .and_then(|v| v.expected.as_ref())
        .map_or_else(|| "?".to_owned(), show);
    match (&cap.verification, r.state) {
        (Some(Verification::Readback { .. }), CommandState::Verified) => {
            Finding::pass(format!("VERIFIED: '{rb}' read back {observed}"))
        }
        (Some(Verification::Readback { .. }), CommandState::VerificationFailed) => {
            Finding::fail(format!(
                "VERIFICATION_FAILED: '{rb}' read back {observed}, expected {expected}; the device did not do what the adapter reported, or the tolerance is too tight"
            ))
            .with(evidence(r))
        }
        (Some(Verification::Readback { .. }), _) => Finding::fail(format!(
            "{}: no usable reading of '{rb}' within the verification timeout",
            wire(&r.state)
        ))
        .with(evidence(r)),
        (Some(Verification::None { .. }), CommandState::Unverifiable) => Finding::pass(
            "UNVERIFIABLE, as declared (mode: none): the receipt claims nothing beyond the device's report",
        ),
        (_, s) => Finding::fail(format!("ended {}, which does not match the declared verification", wire(&s)))
            .with(evidence(r)),
    }
}
