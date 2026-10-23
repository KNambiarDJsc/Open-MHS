//! A fake lamp for tests: the same line protocol as the real one, with faults on demand.
//!
//! It stands where the device would be, so the conformance suite tests the adapter's real
//! parsing and error mapping. Serve it in memory ([`Link`]) or over TCP
//! ([`FakeLamp::serve`]) to exercise [`TcpLink`](crate::link::TcpLink) too. When you
//! change the protocol in `lib.rs`, change it here as well.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use openmhs_conformance::{Fault, Fixture};
use openmhs_types::Source;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::link::{Link, LinkError};

#[derive(Debug)]
struct Lamp {
    brightness: f64,
    on: bool,
    flashes: i64,
    link_down: bool,
    fault: Option<Fault>,
    /// State-changing requests received, applied or not.
    deliveries: u64,
}

/// What the lamp does with one request line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Line(String),
    /// No answer at all.
    Silent,
}

pub struct FakeLamp {
    lamp: Mutex<Lamp>,
}

impl FakeLamp {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            lamp: Mutex::new(Lamp {
                brightness: 50.0,
                on: false,
                flashes: 0,
                link_down: false,
                fault: None,
                deliveries: 0,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Lamp> {
        self.lamp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn link_down(&self) -> bool {
        self.lock().link_down
    }

    /// Handle one request line as the lamp would.
    pub fn respond(&self, request: &str) -> Reply {
        let mut lamp = self.lock();
        let (verb, arg) = match request.trim().split_once(' ') {
            Some((v, a)) => (v, Some(a.trim())),
            None => (request.trim(), None),
        };
        let answer = |s: String| Reply::Line(s);
        if verb.ends_with('?') {
            return answer(match verb {
                "BRIGHT?" => format!("{:.1}", lamp.brightness),
                "ON?" => u8::from(lamp.on).to_string(),
                "POWER?" => format!("{:.2}", if lamp.on { lamp.brightness * 0.6 } else { 0.0 }),
                "FLASHES?" => lamp.flashes.to_string(),
                _ => "ERR unknown query".into(),
            });
        }

        // A state-changing request reached the device.
        lamp.deliveries += 1;
        let fault = lamp.fault;
        match fault {
            Some(Fault::Refuse) => return answer("ERR refused by the device".into()),
            Some(Fault::NoResponse) => return Reply::Silent,
            _ => {}
        }
        let outcome = apply(&mut lamp, verb, arg);
        match (fault, outcome) {
            (Some(Fault::LostAck), Ok(())) => Reply::Silent,
            (Some(Fault::Garbled), Ok(())) => answer("\u{1}#?K".into()),
            (_, Ok(())) => answer("OK".into()),
            (_, Err(reason)) => answer(format!("ERR {reason}")),
        }
    }

    /// Serve the lamp on `listener` until the task is dropped. One line in, one line out.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        while let Ok((stream, _)) = listener.accept().await {
            let lamp = self.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if lamp.link_down() {
                        return; // dropping the connection is what a dead link looks like
                    }
                    match lamp.respond(&line) {
                        Reply::Line(reply) => {
                            if write
                                .write_all(format!("{reply}\n").as_bytes())
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Reply::Silent => {}
                    }
                }
            });
        }
    }
}

fn apply(lamp: &mut Lamp, verb: &str, arg: Option<&str>) -> Result<(), String> {
    match (verb, arg) {
        ("BRIGHT", Some(a)) => match a.parse::<f64>() {
            Ok(b) if (0.0..=100.0).contains(&b) => {
                lamp.brightness = b;
                Ok(())
            }
            _ => Err("brightness must be 0..100".into()),
        },
        ("ON", Some("1")) => {
            lamp.on = true;
            Ok(())
        }
        ("ON", Some("0")) => {
            lamp.on = false;
            Ok(())
        }
        ("FLASH", Some(a)) => match a.parse::<i64>() {
            Ok(n) if (1..=10).contains(&n) && lamp.on => {
                lamp.flashes += n;
                Ok(())
            }
            Ok(_) if !lamp.on => Err("the lamp is off".into()),
            _ => Err("count must be 1..10".into()),
        },
        _ => Err("unknown command".into()),
    }
}

/// The fake lamp as an in-memory link.
#[async_trait]
impl Link for FakeLamp {
    async fn transact(&self, request: &str, timeout: Duration) -> Result<String, LinkError> {
        if self.link_down() {
            return Err(LinkError::Down("the fake lamp's link is down".into()));
        }
        match self.respond(request) {
            Reply::Line(l) => Ok(l),
            Reply::Silent => {
                tokio::time::sleep(timeout).await;
                Err(LinkError::Timeout)
            }
        }
    }

    fn source(&self) -> Source {
        Source::Simulator
    }
}

#[async_trait]
impl Fixture for FakeLamp {
    fn supports(&self, _fault: Fault) -> bool {
        true
    }

    async fn inject(&self, fault: Fault) -> Result<(), String> {
        let mut lamp = self.lock();
        match fault {
            Fault::LinkDown => lamp.link_down = true,
            other => lamp.fault = Some(other),
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), String> {
        let mut lamp = self.lock();
        lamp.link_down = false;
        lamp.fault = None;
        Ok(())
    }

    fn deliveries(&self) -> Option<u64> {
        Some(self.lock().deliveries)
    }
}
