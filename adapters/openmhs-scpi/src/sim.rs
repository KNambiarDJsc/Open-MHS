//! A simulated SCPI instrument that speaks a profile's command set, over TCP or any
//! stream (a serial port). Requests are understood through the same profile the adapter
//! uses, read in reverse, and act on a [`VirtualDevice`] with conformance faults.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use openmhs_profile::{Outcome, VirtualDevice, codec, template};
use openmhs_types::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::{Profile, decode, encode};

/// What a garbled answer looks like.
const GARBAGE: &str = "\u{1}#?K";

pub struct ScpiSim {
    profile: Profile,
    device: Arc<VirtualDevice>,
    errors: Mutex<VecDeque<(i32, String)>>,
}

impl ScpiSim {
    /// An instrument for `profile`, starting in the profile's `simulator.initial` state.
    pub fn new(profile: Profile) -> Arc<Self> {
        let device = VirtualDevice::new(profile.descriptor.clone(), &profile.simulator.initial);
        Arc::new(Self {
            profile,
            device,
            errors: Mutex::new(VecDeque::new()),
        })
    }

    /// The device behind the protocol: its state, and the conformance fixture.
    pub fn device(&self) -> Arc<VirtualDevice> {
        self.device.clone()
    }

    fn errors(&self) -> MutexGuard<'_, VecDeque<(i32, String)>> {
        self.errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn push_error(&self, code: i32, message: String) {
        let mut q = self.errors();
        if q.len() < 32 {
            q.push_back((code, message));
        }
    }

    /// Handle one request line. `None`: the instrument does not answer.
    pub fn handle(&self, line: &str) -> Option<String> {
        let c = &self.profile.connection;
        let mut replies = Vec::new();
        let mut silent = false;
        let mut garbled = false;
        for part in line.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            if part.eq_ignore_ascii_case(&c.identify) {
                let d = &self.profile.descriptor.device;
                replies.push(format!("{},{},{},0.1", d.manufacturer, d.model, d.id));
            } else if part.eq_ignore_ascii_case(&c.completion) {
                replies.push("1".to_owned());
            } else if part.eq_ignore_ascii_case(&c.error_query) {
                let (code, message) = self.errors().pop_front().unwrap_or((0, "No error".into()));
                replies.push(format!("{code},\"{message}\""));
            } else if part.eq_ignore_ascii_case("*CLS") {
                self.errors().clear();
            } else if let Some(reply) = self.query(part) {
                replies.push(reply);
            } else {
                match self.command(part) {
                    Some(Outcome::Applied) => {}
                    Some(Outcome::Refused(reason)) => {
                        self.push_error(-221, format!("Settings conflict; {reason}"))
                    }
                    Some(Outcome::Silent) => silent = true,
                    Some(Outcome::Garbled) => garbled = true,
                    None => self.push_error(-113, format!("Undefined header; {part}")),
                }
            }
        }
        if silent || replies.is_empty() {
            return None;
        }
        Some(if garbled {
            GARBAGE.to_owned()
        } else {
            replies.join(";")
        })
    }

    fn query(&self, part: &str) -> Option<String> {
        self.profile.mapping.iter().find_map(|(name, m)| {
            let q = m.query.as_deref()?;
            q.eq_ignore_ascii_case(part).then(|| {
                self.device
                    .read(name)
                    .map(|v| encode(m, &v))
                    .unwrap_or_default()
            })
        })
    }

    /// A state-changing command, or `None` if no mapping matches it.
    fn command(&self, part: &str) -> Option<Outcome> {
        for (name, m) in &self.profile.mapping {
            let cap = self.profile.descriptor.capability(name)?;
            if let (Some(set), Some(spec)) = (&m.set, &cap.value)
                && let Some(found) = template::match_text(set, part)
            {
                let text = found.get("value").map_or("", String::as_str);
                return Some(match decode(m, spec, text) {
                    Ok(v) => self.device.command(name, Some(v), &BTreeMap::new()),
                    Err(e) => Outcome::Refused(format!("Data out of range; {e}")),
                });
            }
            if let Some(send) = &m.send
                && let Some(found) = template::match_text(send, part)
            {
                let mut params = BTreeMap::new();
                for (p, spec) in cap.params.iter().flatten() {
                    let Some(text) = found.get(p) else { continue };
                    match codec::from_text(spec, text) {
                        Ok(v) => {
                            params.insert(p.clone(), v);
                        }
                        Err(e) => return Some(Outcome::Refused(format!("{p}: {e}"))),
                    }
                }
                return Some(self.device.command(name, None, &params));
            }
        }
        None
    }

    /// Change a value as the physical world would, e.g. a measurement drifting.
    pub fn set(&self, capability: &str, value: Value) {
        self.device.set(capability, value);
    }

    /// Serve on `listener` until the task is dropped. While the link is down,
    /// connections are dropped, which is what a dead link looks like over TCP.
    pub async fn serve_tcp(self: Arc<Self>, listener: TcpListener) {
        while let Ok((stream, _)) = listener.accept().await {
            if self.device.link_down() {
                continue;
            }
            let sim = self.clone();
            tokio::spawn(async move { sim.serve(stream, true).await });
        }
    }

    /// Serve one stream, such as a serial port. While the link is down the instrument
    /// does not answer.
    pub async fn serve_stream<S: AsyncRead + AsyncWrite + Unpin>(self: Arc<Self>, stream: S) {
        self.serve(stream, false).await;
    }

    async fn serve<S: AsyncRead + AsyncWrite + Unpin>(&self, stream: S, hang_up_when_down: bool) {
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match stream.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if self.device.link_down() {
                if hang_up_when_down {
                    return;
                }
                continue;
            }
            if let Some(reply) = self.handle(line.trim_end_matches(['\r', '\n'])) {
                let out = stream.get_mut();
                if out
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                    || out.flush().await.is_err()
                {
                    return;
                }
            }
        }
    }
}
