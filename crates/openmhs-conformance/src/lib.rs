//! The OpenMHS conformance suite.
//!
//! Runs an adapter, together with a [`Fixture`] that can inject faults where the device
//! would be, through the requirements of `spec/conformance.md`, and produces a
//! machine-readable [`Report`]: every check with its status, and the level reached.
//!
//! ```no_run
//! # async fn demo(adapter: std::sync::Arc<dyn openmhs_adapter::HardwareAdapter>,
//! #               fixture: std::sync::Arc<dyn openmhs_conformance::Fixture>) {
//! let report = openmhs_conformance::run(adapter, fixture, Default::default()).await;
//! assert!(report.passed, "{}", openmhs_conformance::render(&report));
//! # }
//! ```
//!
//! A conformance level is not a safety certification. Commands are really sent, so run
//! the suite against fixtures and simulators, not hardware that could hurt someone.

mod fixture;
mod render;
mod report;
mod requirements;
mod runner;
pub mod sim;
mod values;

use std::sync::Arc;

use openmhs_adapter::HardwareAdapter;

pub use fixture::{Fault, Fixture, NoFixture};
pub use openmhs_types::descriptor::ConformanceLevel;
pub use render::render;
pub use report::{
    CheckResult, Environment, FixtureInfo, REPORT_VERSION, Report, Status, Subject, Summary,
    achieved_level,
};
pub use requirements::{REQUIREMENTS, Requirement, requirement};
pub use runner::{Options, PRINCIPAL};

/// Run every requirement against `adapter`, injecting faults through `fixture`.
pub async fn run(
    adapter: Arc<dyn HardwareAdapter>,
    fixture: Arc<dyn Fixture>,
    options: Options,
) -> Report {
    runner::Runner::new(adapter, fixture, options).run().await
}
