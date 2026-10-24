//! A simulated HTTP device serving a profile's API.
//!
//! A small HTTP/1.1 server written for the purpose, so it can misbehave in ways a web
//! framework would not allow: hold a request without answering, apply a command and then
//! go silent, answer with garbage, or drop connections while the link is down.

use std::collections::BTreeMap;
use std::sync::Arc;

use openmhs_profile::template::{self, put};
use openmhs_profile::{Outcome, VirtualDevice, codec};
use openmhs_types::{Clock, SystemClock};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::Profile;

enum Reply {
    Json(u16, serde_json::Value),
    Raw(u16, String),
    Silent,
}

pub struct HttpSim {
    profile: Profile,
    device: Arc<VirtualDevice>,
}

impl HttpSim {
    pub fn new(profile: Profile) -> Arc<Self> {
        let device = VirtualDevice::new(profile.descriptor.clone(), &profile.simulator.initial);
        Arc::new(Self { profile, device })
    }

    /// The device behind the protocol: its state, and the conformance fixture.
    pub fn device(&self) -> Arc<VirtualDevice> {
        self.device.clone()
    }

    /// Serve on `listener` until the task is dropped.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        while let Ok((stream, _)) = listener.accept().await {
            if self.device.link_down() {
                continue;
            }
            let sim = self.clone();
            tokio::spawn(async move { sim.connection(stream).await });
        }
    }

    /// Serve HTTPS on `listener` with `config` (for the TLS tests). A client whose TLS
    /// handshake fails is dropped.
    pub async fn serve_tls(
        self: Arc<Self>,
        listener: TcpListener,
        config: Arc<rustls::ServerConfig>,
    ) {
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        while let Ok((stream, _)) = listener.accept().await {
            if self.device.link_down() {
                continue;
            }
            let (sim, acceptor) = (self.clone(), acceptor.clone());
            tokio::spawn(async move {
                if let Ok(stream) = acceptor.accept(stream).await {
                    sim.connection(stream).await;
                }
            });
        }
    }

    async fn connection<S: AsyncRead + AsyncWrite + Unpin>(&self, stream: S) {
        let mut stream = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if matches!(stream.read_line(&mut line).await, Ok(0) | Err(_)) {
                return;
            }
            let mut parts = line.split_whitespace();
            let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
                return;
            };
            let (method, path) = (
                method.to_owned(),
                target.split('?').next().unwrap_or("").to_owned(),
            );
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if matches!(stream.read_line(&mut header).await, Ok(0) | Err(_)) {
                    return;
                }
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some((k, v)) = header.split_once(':')
                    && k.trim().eq_ignore_ascii_case("content-length")
                {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length.min(1 << 20)];
            if stream.read_exact(&mut body).await.is_err() || self.device.link_down() {
                return;
            }
            let (status, text) = match self.respond(&method, &path, &body) {
                Reply::Silent => {
                    // Hold the request until the client gives up.
                    let mut sink = Vec::new();
                    let _ = stream.read_to_end(&mut sink).await;
                    return;
                }
                Reply::Json(status, json) => (status, json.to_string()),
                Reply::Raw(status, text) => (status, text),
            };
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                404 => "Not Found",
                409 => "Conflict",
                _ => "Status",
            };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                text.len()
            );
            let out = stream.get_mut();
            if out.write_all(head.as_bytes()).await.is_err()
                || out.write_all(text.as_bytes()).await.is_err()
            {
                return;
            }
        }
    }

    fn respond(&self, method: &str, path: &str, body: &[u8]) -> Reply {
        if method.eq_ignore_ascii_case("GET") {
            return self.document(path);
        }
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) else {
            return self.refusal(400, "the body is not JSON".into());
        };
        for (name, ep) in &self.profile.mapping {
            let Some(cap) = self.profile.descriptor.capability(name) else {
                continue;
            };
            for (rule, is_write) in [(&ep.write, true), (&ep.invoke, false)] {
                let Some(rule) = rule else { continue };
                if !rule.method.as_str().eq_ignore_ascii_case(method) || rule.path != path {
                    continue;
                }
                let Ok(slots) = template::json_slots(&rule.body) else {
                    continue;
                };
                if !slots.iter().all(|(_, p)| json.pointer(p).is_some()) {
                    continue; // another capability shares this endpoint
                }
                let mut value = None;
                let mut params = BTreeMap::new();
                for (slot, pointer) in &slots {
                    let spec = if is_write && slot == "value" {
                        cap.value.as_ref()
                    } else {
                        cap.params.as_ref().and_then(|p| p.get(slot))
                    };
                    let (Some(spec), Some(raw)) = (spec, json.pointer(pointer)) else {
                        continue;
                    };
                    match codec::from_json(spec, raw) {
                        Ok(v) if is_write => value = Some(v),
                        Ok(v) => {
                            params.insert(slot.clone(), v);
                        }
                        Err(e) => return self.refusal(400, format!("{slot}: {e}")),
                    }
                }
                return match self.device.command(name, value, &params) {
                    Outcome::Applied => self.success(),
                    Outcome::Refused(reason) => self.refusal(409, reason),
                    Outcome::Silent => Reply::Silent,
                    Outcome::Garbled => Reply::Raw(200, "\u{1}#?K".into()),
                };
            }
        }
        self.refusal(404, format!("no {method} {path}"))
    }

    /// The JSON document a GET of `path` returns: every property read from there.
    fn document(&self, path: &str) -> Reply {
        let mut doc = serde_json::json!({});
        let mut found = false;
        for (name, ep) in &self.profile.mapping {
            let Some(rule) = ep.read.as_ref().filter(|r| r.path == path) else {
                continue;
            };
            found = true;
            if let Some(v) = self.device.read(name) {
                put(&mut doc, &rule.pointer, codec::to_json(&v));
            }
            if let Some(t) = &rule.timestamp_pointer {
                put(
                    &mut doc,
                    t,
                    serde_json::json!(SystemClock.now().to_rfc3339()),
                );
            }
        }
        if found {
            Reply::Json(200, doc)
        } else {
            self.refusal(404, format!("no GET {path}"))
        }
    }

    fn success(&self) -> Reply {
        let mut doc = serde_json::json!({});
        if let Some(r) = &self.profile.connection.response {
            put(&mut doc, &r.ok_pointer, serde_json::json!(true));
        }
        Reply::Json(200, doc)
    }

    fn refusal(&self, status: u16, reason: String) -> Reply {
        let mut doc = serde_json::json!({});
        if let Some(r) = &self.profile.connection.response {
            put(&mut doc, &r.ok_pointer, serde_json::json!(false));
            if let Some(e) = &r.error_pointer {
                put(&mut doc, e, serde_json::json!(reason));
            }
        }
        Reply::Json(status, doc)
    }
}
