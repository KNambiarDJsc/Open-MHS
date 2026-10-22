//! Simulated devices for OpenMHS.
//!
//! Simulators implement the same [`HardwareAdapter`](openmhs_adapter::HardwareAdapter)
//! contract as real adapters and go through the same engine, policy and journal
//! (invariant 9). Behaviour is deterministic for a given seed and clock.

mod axis;
mod fault;
mod psu;
mod rng;
mod thermo;

use std::sync::Arc;

use openmhs_adapter::HardwareAdapter;
use openmhs_types::Clock;

pub use axis::{AxisHandle, AxisTruth, Motion, MotionKind, VirtualAxis};
pub use fault::{ExecFault, Faults, GENERIC_FAULTS, SimControl, apply_fault};
pub use psu::{PsuHandle, PsuTruth, VirtualPsu};
pub use thermo::{ThermoHandle, VirtualThermo};

/// A simulator ready to register: the adapter, and the control handle for faults.
pub struct SimDevice {
    pub adapter: Arc<dyn HardwareAdapter>,
    pub control: Arc<dyn SimControl>,
}

/// One of each simulated device class: a temperature sensor (read-only), a power supply
/// (writable properties) and a linear stage (actions, hazardous motion).
pub fn virtual_lab(clock: Arc<dyn Clock>, seed: u64) -> Vec<SimDevice> {
    let (thermo, thermo_h) = VirtualThermo::new(clock.clone(), seed);
    let (psu, psu_h) = VirtualPsu::new(clock.clone(), seed);
    let (axis, axis_h) = VirtualAxis::new(clock);
    vec![
        SimDevice {
            adapter: Arc::new(thermo),
            control: Arc::new(thermo_h),
        },
        SimDevice {
            adapter: Arc::new(psu),
            control: Arc::new(psu_h),
        },
        SimDevice {
            adapter: Arc::new(axis),
            control: Arc::new(axis_h),
        },
    ]
}
