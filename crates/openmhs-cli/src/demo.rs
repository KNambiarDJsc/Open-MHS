//! `openmhs demo`: a guided tour against a throwaway gateway and the virtual lab.
//!
//! The demo talks to the gateway over the real local API, exactly as any client would.
//! Each step states what should happen and checks that it did; the demo exits non-zero
//! if anything deviates, so CI runs it as an end-to-end test.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use openmhs_api::{Client, ClientError, CreateTokenParams, Role};
use openmhs_gateway::GatewayConfig;
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::{Operation, SideEffect};
use openmhs_types::{CommandState, Receipt};
use serde_json::{Value, json};

use crate::exit;
use crate::output;

const AGENT: &str = "agent:demo";
const PSU: &str = "sim.psu-01";
const AXIS: &str = "sim.axis-01";
const THERMO: &str = "sim.thermo-01";
const SETPOINT: &str = "power.output.voltage.setpoint";
/// The supply's slew window (2 V per 1000 ms) plus a margin.
const SLEW_WINDOW: std::time::Duration = std::time::Duration::from_millis(1_100);

struct Tour {
    step: u32,
    failures: Vec<String>,
}

impl Tour {
    fn step(&mut self, title: &str) {
        self.step += 1;
        println!();
        println!("[{}] {title}", self.step);
    }

    fn say(&self, text: &str) {
        println!("    {text}");
    }

    fn check(&mut self, ok: bool, what: &str) {
        if ok {
            println!("    ok: {what}");
        } else {
            println!("    UNEXPECTED: {what}");
            self.failures.push(format!("step {}: {what}", self.step));
        }
    }

    fn receipt(&mut self, r: &Receipt, expect: CommandState, what: &str) {
        let first_error = r
            .policy
            .errors
            .first()
            .map(|e| format!("  [{}] {}", e.code, e.message))
            .unwrap_or_default();
        let verify = r
            .verification
            .as_ref()
            .and_then(|v| v.observed.as_ref())
            .map(|o| format!("  readback {}", output::value(&o.value)))
            .unwrap_or_default();
        println!(
            "    -> {}{first_error}{verify}   (command {})",
            output::name(&r.state),
            short(&r.command_id)
        );
        self.check(r.state == expect, what);
    }

    fn denied_with(&mut self, r: &Receipt, code: &str) {
        let has = r.policy.errors.iter().any(|e| e.code == code);
        self.receipt(
            r,
            CommandState::Denied,
            "denied; nothing was sent to the device",
        );
        if r.state == CommandState::Denied {
            self.check(has, &format!("reason is {code}"));
        }
    }
}

fn short(id: &str) -> &str {
    id.get(id.len().saturating_sub(12)..).unwrap_or(id)
}

fn write(value: Value) -> Value {
    json!({ "target": { "device": PSU, "capability": SETPOINT }, "operation": "write", "value": value })
}

fn volts(v: f64) -> Value {
    json!({ "value": v, "unit": "V" })
}

fn grant(
    id: &str,
    device: &str,
    capability: &str,
    ops: &[Operation],
    effect: Effect,
    max: Option<SideEffect>,
) -> Grant {
    Grant {
        id: id.into(),
        principal: "agent:*".into(),
        device: device.into(),
        capability: capability.into(),
        operations: ops.to_vec(),
        effect,
        max_side_effect: max,
        reason: None,
    }
}

fn temp_home() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("openmhs-demo-{}-{stamp}", std::process::id()))
}

pub async fn run(keep: bool) -> u8 {
    // The crash step panics inside an adapter on purpose. Say so briefly instead of
    // printing a raw panic message in the middle of the tour.
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .unwrap_or("panic");
        println!("    (adapter panicked: \"{msg}\"; contained by the gateway)");
    }));
    println!("OpenMHS demo: simulated devices only; no real hardware is touched.");
    println!("OpenMHS is an independent open-source project, not affiliated with Anthropic.");
    let home = temp_home();
    let mut config = GatewayConfig::new(&home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    let gateway = match openmhs_gateway::start(config).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: cannot start the demo gateway: {e}");
            return exit::ERROR;
        }
    };
    let mut tour = Tour {
        step: 0,
        failures: Vec::new(),
    };
    let result = scenario(&mut tour, &gateway.url, &gateway.operator_token).await;
    if let Err(e) = gateway.shutdown().await {
        eprintln!("warning: gateway shutdown: {e}");
    }
    if let Err(e) = result {
        tour.failures.push(format!("API error: {e}"));
    }

    println!();
    if keep {
        println!("Demo home kept at {}", home.display());
        println!(
            "  inspect it with: openmhs serve --sim --home \"{}\"",
            home.display()
        );
    } else {
        let _ = std::fs::remove_dir_all(&home);
    }
    if tour.failures.is_empty() {
        println!("Demo complete: every step behaved as expected.");
        exit::OK
    } else {
        println!("Demo FAILED:");
        for f in &tour.failures {
            println!("  - {f}");
        }
        exit::FAILED
    }
}

async fn scenario(t: &mut Tour, url: &str, operator_token: &str) -> Result<(), ClientError> {
    let operator = Client::new(url, operator_token)?;

    t.step("Gateway started with three simulated devices");
    t.say(&format!("listening on {url} (loopback only)"));
    let devices = operator.devices().await?;
    for d in &devices {
        t.say(&format!(
            "{:<14} {}",
            d.device.id,
            d.device.name.as_deref().unwrap_or("")
        ));
    }
    t.check(devices.len() == 3, "three devices registered");

    t.step("The operator issues a token for the agent principal 'agent:demo'");
    let token = operator
        .create_token(CreateTokenParams::unrestricted(AGENT, Role::Agent))
        .await?;
    let agent = Client::new(url, &token.token)?;
    let me = agent.info().await?;
    t.check(
        me.principal == AGENT && me.role == Role::Agent,
        "the agent is authenticated as agent:demo",
    );

    t.step("No grants yet: the agent asks for 5 V and is refused (deny by default)");
    let r = agent.submit(write(volts(5.0))).await?;
    t.denied_with(&r, "policy.denied.no_grant");

    t.step("The agent tries to grant itself access");
    let selfgrant = agent
        .add_grant(grant(
            "sneaky",
            "*",
            "*",
            &[Operation::Write],
            Effect::Allow,
            None,
        ))
        .await;
    t.check(
        matches!(&selfgrant, Err(e) if e.code() == Some("gateway.forbidden")),
        "refused: only operators change policy",
    );

    t.step("The operator grants the agent a narrow set of permissions");
    for g in [
        grant(
            "agent-read",
            "*",
            "*",
            &[Operation::Read],
            Effect::Allow,
            None,
        ),
        grant(
            "agent-psu",
            PSU,
            "power.output.*",
            &[Operation::Write],
            Effect::Allow,
            Some(SideEffect::Reversible),
        ),
        grant(
            "agent-axis-home",
            AXIS,
            "axis.home",
            &[Operation::Invoke],
            Effect::Allow,
            Some(SideEffect::Reversible),
        ),
        grant(
            "agent-axis-move",
            AXIS,
            "axis.move_absolute",
            &[Operation::Invoke],
            Effect::RequireApproval,
            Some(SideEffect::Hazardous),
        ),
    ] {
        let g = operator.add_grant(g).await?;
        t.say(&format!(
            "{:<16} {} {} / {}",
            g.id,
            output::name(&g.effect),
            g.device,
            g.capability
        ));
    }

    t.step("The agent reads the temperature (authorized, audited, timestamped)");
    let r = agent.read(THERMO, "temperature.ambient").await?;
    t.say(&format!(
        "{} = {} ({}, {} ms old)",
        r.capability,
        output::value(&r.state.value),
        output::name(&r.state.quality),
        r.age_ms
    ));
    t.check(r.usable, "reading is fresh and GOOD");

    t.step("The agent asks for 48 V; the supply's range is 0..12 V");
    let r = agent.submit(write(volts(48.0))).await?;
    t.denied_with(&r, "command.value.out_of_range");

    t.step("The agent sends a bare number 5 with no unit; units are never guessed");
    let r = agent.submit(write(json!(5))).await?;
    t.denied_with(&r, "command.value.type_mismatch");

    t.step("The agent asks to jump straight from 0 V to 5 V");
    t.say("the supply declares a slew limit of 2 V per second");
    let r = agent.submit(write(volts(5.0))).await?;
    t.denied_with(&r, "command.slew_exceeded");

    t.step("The agent ramps instead: 2 V, then 4 V, then 5 V, one second apart");
    for v in [2.0, 4.0] {
        let r = agent.submit(write(volts(v))).await?;
        t.receipt(
            &r,
            CommandState::Verified,
            &format!("{v} V applied and confirmed by readback"),
        );
        tokio::time::sleep(SLEW_WINDOW).await;
    }
    let mut req = write(volts(5.0));
    req["idempotency_key"] = "demo-5v".into();
    let first = agent.submit(req.clone()).await?;
    t.receipt(
        &first,
        CommandState::Verified,
        "5 V applied and confirmed by readback",
    );

    t.step("A network blip makes the agent retry the 5 V request (same idempotency key)");
    let again = agent.submit(req).await?;
    t.check(
        again.command_id == first.command_id,
        "the gateway returns the original command instead of running it again",
    );

    t.step("The supply's telemetry goes stale (simulated 5 s delay); the agent asks for 6 V");
    operator.sim_fault(PSU, "stale=5000").await?;
    let r = agent.submit(write(volts(6.0))).await?;
    t.denied_with(&r, "command.state.stale");
    operator.sim_fault(PSU, "clear").await?;

    t.step("The agent homes the axis (reversible, allowed)");
    let home =
        json!({ "target": { "device": AXIS, "capability": "axis.home" }, "operation": "invoke" });
    let r = agent.submit(home).await?;
    t.receipt(&r, CommandState::Verified, "homed; axis.homed reads true");

    t.step("The agent requests a hazardous move to 120 mm at 50 mm/s");
    let mv = json!({
        "target": { "device": AXIS, "capability": "axis.move_absolute" },
        "operation": "invoke",
        "params": {
            "position": { "value": 120, "unit": "mm" },
            "speed": { "value": 50, "unit": "mm/s" }
        }
    });
    let pending = agent.submit(mv).await?;
    t.receipt(
        &pending,
        CommandState::AwaitingApproval,
        "held for human approval; nothing moved",
    );

    t.step("The agent tries to approve its own move");
    let own = agent.approve(&pending.command_id).await;
    t.check(
        matches!(&own, Err(e) if e.code() == Some("gateway.forbidden")),
        "refused: agents cannot approve",
    );

    t.step("The operator approves; preconditions are re-checked and the carriage moves");
    let r = operator.approve(&pending.command_id).await?;
    t.receipt(
        &r,
        CommandState::Verified,
        "position read back within 0.1 mm of 120 mm",
    );

    t.step("Lost acknowledgement: the supply applies 7 V but never answers (3 s deadline)");
    tokio::time::sleep(SLEW_WINDOW).await;
    operator.sim_fault(PSU, "apply-then-hang").await?;
    let mut req = write(volts(7.0));
    req["deadline_ms"] = 3000.into();
    let lost = agent.submit(req).await?;
    t.receipt(
        &lost,
        CommandState::UnknownOutcome,
        "UNKNOWN_OUTCOME: not reported as failed, and not retried",
    );

    t.step("The fault clears; the agent reconciles by reading the device");
    operator.sim_fault(PSU, "clear").await?;
    let r = agent.reconcile(&lost.command_id).await?;
    t.receipt(
        &r,
        CommandState::ReconciledApplied,
        "readback shows 7 V: it did take effect",
    );

    t.step("The supply's adapter crashes (panics) while handling a 6 V request");
    tokio::time::sleep(SLEW_WINDOW).await;
    operator.sim_fault(PSU, "crash").await?;
    let crashed = agent.submit(write(volts(6.0))).await?;
    t.receipt(
        &crashed,
        CommandState::UnknownOutcome,
        "the crash is contained and classified; the outcome is uncertain",
    );
    operator.sim_fault(PSU, "clear").await?;
    let info = operator.info().await?;
    t.check(info.devices == 3, "the gateway is still serving");
    let r = agent.reconcile(&crashed.command_id).await?;
    t.receipt(
        &r,
        CommandState::ReconciledNotApplied,
        "readback still shows 7 V: the 6 V request never took effect",
    );

    t.step("The operator verifies the audit log");
    let v = operator.verify_audit().await?;
    t.say(&format!(
        "{} records; every grant, read, transition and injected fault is recorded",
        v.records
    ));
    t.check(v.valid, "hash chain intact");
    Ok(())
}
