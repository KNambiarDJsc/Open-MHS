//! An OpenMHS adapter for an example lamp, and the starting point for yours.
//!
//! The gateway does the hard parts before your code runs: authorization, units and
//! ranges, rate and slew limits, preconditions, the journal, readback verification. An
//! adapter does three things, and does them honestly:
//!
//! 1. **Describe** the device (`descriptor.json`).
//! 2. **Read** properties, stamping each reading with when it was taken and where it came
//!    from.
//! 3. **Execute** writes and actions, reporting only what the device said, and classifying
//!    every failure so the gateway knows whether the device might have acted.
//!
//! Never retry a state-changing request yourself, and never report success you did not
//! observe. The conformance suite (`tests/conformance.rs`) checks all of this against the
//! fake lamp in `fixture.rs`.

pub mod fixture;
pub mod link;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_types::{Clock, Quality, Quantity, StateValue, Value};

use crate::link::{Link, LinkError};

/// Must equal `adapter.id` in the descriptor.
pub const ADAPTER_ID: &str = "example.lamp";

/// The device descriptor: what the device is and what it can do. Agents and the gateway
/// trust nothing that is not declared here.
pub const DESCRIPTOR: &str = include_str!("../descriptor.json");

pub struct LampAdapter {
    link: Arc<dyn Link>,
    clock: Arc<dyn Clock>,
    /// How long to wait for any one reply.
    reply_timeout: Duration,
}

impl LampAdapter {
    /// `clock` timestamps readings: `openmhs_types::SystemClock` in production.
    pub fn new(link: Arc<dyn Link>, clock: Arc<dyn Clock>) -> Self {
        Self {
            link,
            clock,
            reply_timeout: Duration::from_millis(500),
        }
    }

    #[must_use]
    pub fn with_reply_timeout(mut self, timeout: Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// A query that changes nothing. Any failure means the reading is unavailable.
    async fn query(&self, request: &str) -> Result<String, AdapterError> {
        self.link
            .transact(request, self.reply_timeout)
            .await
            .map_err(|e| match e {
                LinkError::Timeout => AdapterError::new(AdapterErrorKind::Timeout, e.to_string()),
                LinkError::Down(_) | LinkError::Lost(_) => {
                    AdapterError::new(AdapterErrorKind::Disconnected, e.to_string())
                }
            })
    }
}

fn malformed(reply: &str) -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::MalformedResponse,
        format!("unexpected reply {reply:?}"),
    )
}

fn number(reply: &str) -> Result<f64, AdapterError> {
    reply
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| malformed(reply))
}

fn integer(reply: &str) -> Result<i64, AdapterError> {
    reply.trim().parse().map_err(|_| malformed(reply))
}

fn flag(reply: &str) -> Result<bool, AdapterError> {
    match reply.trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(malformed(reply)),
    }
}

fn quantity(value: f64, unit: &str) -> Value {
    Value::Quantity(Quantity {
        value,
        unit: unit.into(),
    })
}

#[async_trait]
impl HardwareAdapter for LampAdapter {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        serde_json::from_str(DESCRIPTOR)
            .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        // Prove the device answers before the gateway registers it.
        self.query("ON?").await.map(|_| ())
    }

    async fn health(&self) -> Health {
        match self.link.transact("ON?", self.reply_timeout).await {
            Ok(_) => Health::Healthy,
            Err(LinkError::Timeout) => Health::Degraded("the device does not answer".into()),
            Err(e) => Health::Disconnected(e.to_string()),
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        // Values are reported in exactly the unit the descriptor declares.
        let value = match capability {
            "lamp.brightness" => quantity(number(&self.query("BRIGHT?").await?)?, "%"),
            "lamp.on" => Value::Bool(flag(&self.query("ON?").await?)?),
            "lamp.power" => quantity(number(&self.query("POWER?").await?)?, "W"),
            "lamp.flash.count" => Value::Integer(integer(&self.query("FLASHES?").await?)?),
            other => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Unsupported,
                    format!("no readable capability '{other}'"),
                ));
            }
        };
        Ok(StateValue {
            value,
            // The device answered just now. If yours reports when it measured, use that.
            timestamp: self.clock.now(),
            source: self.link.source(),
            quality: Quality::Good,
        })
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        // The gateway has already checked the value's type, unit and range.
        let not_sent = |m: &str| AdapterError::new(AdapterErrorKind::NotSent, m);
        let request = match (command.capability.as_str(), &command.value) {
            ("lamp.brightness", Some(Value::Quantity(q))) => format!("BRIGHT {}", q.value),
            ("lamp.on", Some(Value::Bool(on))) => format!("ON {}", u8::from(*on)),
            ("lamp.flash", None) => match command.params.get("count") {
                Some(Value::Integer(n)) => format!("FLASH {n}"),
                _ => return Err(not_sent("count must be an integer")),
            },
            (other, _) => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Unsupported,
                    format!("cannot execute '{other}'"),
                ));
            }
        };

        // Send once. Whatever happens next is reported, never retried.
        let reply = self
            .link
            .transact(&request, self.reply_timeout)
            .await
            .map_err(|e| match e {
                // Nothing left this machine: safe to report as not sent.
                LinkError::Down(m) => AdapterError::new(AdapterErrorKind::NotSent, m),
                // It may have reached the device: the outcome is unknown.
                LinkError::Lost(m) => AdapterError::new(AdapterErrorKind::Disconnected, m),
                LinkError::Timeout => AdapterError::new(
                    AdapterErrorKind::Timeout,
                    format!("no reply to '{request}'"),
                ),
            })?;

        match reply.trim() {
            "OK" => {
                progress.accepted();
                Ok(ExecuteReport::Applied {
                    result: BTreeMap::new(),
                })
            }
            r if r.starts_with("ERR") => Ok(ExecuteReport::Rejected {
                reason: r.trim_start_matches("ERR").trim().to_owned(),
            }),
            // An answer we cannot read: the device may well have acted.
            r => Err(malformed(r)),
        }
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}
