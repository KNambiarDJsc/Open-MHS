//! The fault boundary between an adapter and the device it drives.
//!
//! Fault behaviour is tested by injecting faults, never assumed (spec/conformance.md). An
//! adapter is tested together with a *fixture*: something that stands where the device
//! would be (a simulator, a fake server speaking the device's protocol) and can be made to
//! misbehave on request. A check that needs a fault the fixture cannot inject is reported
//! `untested`, and a level with untested checks is not reached.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A fault injected between the adapter and the device (spec/conformance.md §3).
///
/// Every fault except [`Fault::LinkDown`] affects only state-changing requests, so the
/// engine's precondition reads and readback verification keep working while it is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// All communication with the device fails, reads included.
    LinkDown,
    /// State-changing requests get no answer and are not applied (a lost command).
    NoResponse,
    /// State-changing requests are applied, but the answer never arrives (a lost
    /// acknowledgement).
    LostAck,
    /// State-changing requests are applied, and the answer cannot be parsed.
    Garbled,
    /// The device refuses state-changing requests and applies nothing.
    Refuse,
}

impl Fault {
    pub const ALL: [Fault; 5] = [
        Fault::LinkDown,
        Fault::NoResponse,
        Fault::LostAck,
        Fault::Garbled,
        Fault::Refuse,
    ];

    /// The wire name, e.g. `no_response`.
    pub fn name(self) -> &'static str {
        match self {
            Fault::LinkDown => "link_down",
            Fault::NoResponse => "no_response",
            Fault::LostAck => "lost_ack",
            Fault::Garbled => "garbled",
            Fault::Refuse => "refuse",
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Stands where the device would be, and misbehaves on request.
#[async_trait]
pub trait Fixture: Send + Sync {
    /// Whether this fixture can inject `fault`.
    fn supports(&self, fault: Fault) -> bool;

    /// Start `fault`. Only called for supported faults, one at a time, and always
    /// followed by [`Fixture::clear`].
    async fn inject(&self, fault: Fault) -> Result<(), String>;

    /// End every fault; the device behaves normally again.
    async fn clear(&self) -> Result<(), String>;

    /// How many state-changing requests have reached the device so far, applied or not,
    /// or `None` if the fixture cannot tell. Used to prove the adapter does not retry.
    fn deliveries(&self) -> Option<u64>;
}

/// For an adapter tested without a fixture. Nothing can be injected, so every fault
/// check is `untested` and the adapter cannot reach L1.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFixture;

#[async_trait]
impl Fixture for NoFixture {
    fn supports(&self, _fault: Fault) -> bool {
        false
    }

    async fn inject(&self, fault: Fault) -> Result<(), String> {
        Err(format!("no fixture: cannot inject {fault}"))
    }

    async fn clear(&self) -> Result<(), String> {
        Ok(())
    }

    fn deliveries(&self) -> Option<u64> {
        None
    }
}
