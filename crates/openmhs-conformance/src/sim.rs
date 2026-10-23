//! Fixtures for the built-in simulators (`openmhs-sim`).
//!
//! A simulator is its own device, so its fixture injects faults through the simulator's
//! fault controls, and counts `execute` calls as deliveries.

use std::sync::Arc;

use async_trait::async_trait;
use openmhs_sim::{SimControl, SimDevice, apply_fault};

use crate::fixture::{Fault, Fixture};
use crate::report::Report;
use crate::runner::Options;

pub struct SimFixture {
    control: Arc<dyn SimControl>,
}

impl SimFixture {
    pub fn new(control: Arc<dyn SimControl>) -> Self {
        Self { control }
    }
}

fn spec(fault: Fault) -> &'static str {
    match fault {
        Fault::LinkDown => "link-down",
        Fault::NoResponse => "hang",
        Fault::LostAck => "apply-then-hang",
        Fault::Garbled => "apply-then-malformed",
        Fault::Refuse => "reject",
    }
}

#[async_trait]
impl Fixture for SimFixture {
    fn supports(&self, _fault: Fault) -> bool {
        true
    }

    async fn inject(&self, fault: Fault) -> Result<(), String> {
        apply_fault(self.control.as_ref(), spec(fault)).map(|_| ())
    }

    async fn clear(&self) -> Result<(), String> {
        apply_fault(self.control.as_ref(), "clear").map(|_| ())
    }

    fn deliveries(&self) -> Option<u64> {
        self.control.execute_calls()
    }
}

/// Run the suite against one simulator. `options.clock` must be the clock the simulator
/// was built with.
pub async fn run(device: &SimDevice, options: Options) -> Report {
    crate::run(
        device.adapter.clone(),
        Arc::new(SimFixture::new(device.control.clone())),
        options,
    )
    .await
}
