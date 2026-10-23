//! Fault injection shared by simulated devices.

use std::collections::BTreeMap;

use openmhs_adapter::{AdapterError, AdapterErrorKind, ExecuteReport};
use openmhs_types::{Clock, Quality, Source, StateValue, Value};

/// How `execute` misbehaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecFault {
    /// Never responds and never applies (lost command).
    Hang,
    /// Applies the change, then never responds (lost acknowledgement).
    ApplyThenHang,
    /// Applies the change, then answers with garbage.
    ApplyThenMalformed,
    /// Refuses the command.
    Reject,
    /// Fails with a disconnect before anything is sent.
    Disconnect,
    /// Panics inside the adapter.
    Crash,
}

/// Active faults. Everything off by default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Faults {
    /// Reads and executes fail with `adapter.disconnected`.
    pub disconnected: bool,
    /// Readings are timestamped this many milliseconds in the past.
    pub stale_by_ms: u64,
    /// Readings carry `BAD` quality.
    pub bad_quality: bool,
    /// Readback of numeric setpoints is offset by this amount (miscalibrated device).
    pub readback_offset: f64,
    /// `execute` misbehaviour.
    pub execute: Option<ExecFault>,
    /// Extra latency added to every `execute`, in milliseconds.
    pub execute_latency_ms: u64,
}

impl Faults {
    pub(crate) fn link(&self) -> Result<(), AdapterError> {
        if self.disconnected {
            Err(AdapterError::new(
                AdapterErrorKind::Disconnected,
                "simulated disconnect",
            ))
        } else {
            Ok(())
        }
    }

    /// A reading as the faulty device would report it.
    pub(crate) fn reading(&self, clock: &dyn Clock, value: Value) -> StateValue {
        StateValue {
            value,
            timestamp: clock.now().minus_ms(self.stale_by_ms),
            source: Source::Simulator,
            quality: if self.bad_quality {
                Quality::Bad
            } else {
                Quality::Good
            },
        }
    }
}

/// Runtime control of a simulator, used by the gateway's `sim.fault` API and the demo.
pub trait SimControl: Send + Sync {
    fn device_id(&self) -> &str;
    fn faults(&self) -> Faults;
    fn set_faults(&self, faults: Faults);
    /// Faults specific to this device model, e.g. `jam` for an axis.
    fn device_faults(&self) -> &'static [&'static str] {
        &[]
    }
    fn inject_device_fault(&self, name: &str) -> Result<(), String> {
        Err(format!("unknown fault '{name}'"))
    }
    /// Clear device-specific faults (generic faults are cleared by [`apply_fault`]).
    fn clear_device_faults(&self) {}
    /// How many times `execute` has been called, if this simulator counts them.
    fn execute_calls(&self) -> Option<u64> {
        None
    }
}

/// Generic fault names accepted by [`apply_fault`], with their argument.
pub const GENERIC_FAULTS: &[&str] = &[
    "clear",
    "link-down",
    "link-up",
    "stale=<ms>",
    "bad-quality",
    "readback-offset=<x>",
    "latency=<ms>",
    "hang",
    "apply-then-hang",
    "apply-then-malformed",
    "reject",
    "disconnect",
    "crash",
];

fn arg<T: std::str::FromStr>(name: &str, arg: Option<&str>) -> Result<T, String> {
    arg.and_then(|a| a.parse().ok())
        .ok_or_else(|| format!("fault '{name}' needs a numeric value, e.g. {name}=1000"))
}

/// Apply a fault given as `name` or `name=value`. Returns a description of what changed.
pub fn apply_fault(sim: &dyn SimControl, spec: &str) -> Result<String, String> {
    let (name, value) = match spec.split_once('=') {
        Some((n, v)) => (n.trim(), Some(v.trim())),
        None => (spec.trim(), None),
    };
    let mut f = sim.faults();
    let exec = |f: &mut Faults, e| f.execute = Some(e);
    match name {
        "clear" => {
            f = Faults::default();
            sim.clear_device_faults();
        }
        "link-down" => f.disconnected = true,
        "link-up" => f.disconnected = false,
        "stale" => f.stale_by_ms = arg(name, value)?,
        "bad-quality" => f.bad_quality = true,
        "readback-offset" => f.readback_offset = arg(name, value)?,
        "latency" => f.execute_latency_ms = arg(name, value)?,
        "hang" => exec(&mut f, ExecFault::Hang),
        "apply-then-hang" => exec(&mut f, ExecFault::ApplyThenHang),
        "apply-then-malformed" => exec(&mut f, ExecFault::ApplyThenMalformed),
        "reject" => exec(&mut f, ExecFault::Reject),
        "disconnect" => exec(&mut f, ExecFault::Disconnect),
        "crash" => exec(&mut f, ExecFault::Crash),
        other if sim.device_faults().contains(&other) => {
            sim.inject_device_fault(other)?;
            return Ok(format!(
                "{}: device fault '{other}' injected",
                sim.device_id()
            ));
        }
        other => {
            let mut known: Vec<&str> = GENERIC_FAULTS.to_vec();
            known.extend_from_slice(sim.device_faults());
            return Err(format!(
                "unknown fault '{other}'; known: {}",
                known.join(", ")
            ));
        }
    }
    sim.set_faults(f.clone());
    Ok(format!("{}: faults now {f:?}", sim.device_id()))
}

/// Fault behaviour before the device applies anything. `Ok(Some(report))` ends `execute`
/// with that report; `Ok(None)` means carry on and apply.
pub(crate) async fn before_apply(
    fault: Option<ExecFault>,
    latency_ms: u64,
) -> Result<Option<ExecuteReport>, AdapterError> {
    tokio::time::sleep(std::time::Duration::from_millis(latency_ms)).await;
    match fault {
        Some(ExecFault::Hang) => std::future::pending().await,
        Some(ExecFault::Disconnect) => Err(AdapterError::new(
            AdapterErrorKind::Disconnected,
            "simulated disconnect",
        )),
        Some(ExecFault::Reject) => Ok(Some(ExecuteReport::Rejected {
            reason: "simulated rejection".into(),
        })),
        Some(ExecFault::Crash) => panic!("simulated adapter crash"),
        _ => Ok(None),
    }
}

/// Fault behaviour after the device applied the change.
pub(crate) async fn after_apply(
    fault: Option<ExecFault>,
    result: BTreeMap<String, Value>,
) -> Result<ExecuteReport, AdapterError> {
    match fault {
        Some(ExecFault::ApplyThenHang) => std::future::pending().await,
        Some(ExecFault::ApplyThenMalformed) => Err(AdapterError::new(
            AdapterErrorKind::MalformedResponse,
            "simulated garbled response",
        )),
        _ => Ok(ExecuteReport::Applied { result }),
    }
}
