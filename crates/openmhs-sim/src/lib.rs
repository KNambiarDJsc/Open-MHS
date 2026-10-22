//! Simulated devices for OpenMHS.
//!
//! Simulators implement the same [`HardwareAdapter`](openmhs_adapter::HardwareAdapter)
//! contract as real adapters and go through the same engine, policy and journal
//! (invariant 9). Behaviour is deterministic for a given seed and clock.

mod fault;
mod psu;
mod rng;

pub use fault::{ExecFault, Faults};
pub use psu::{PsuHandle, PsuTruth, VirtualPsu};
