//! Fault injection shared by simulated devices.

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
