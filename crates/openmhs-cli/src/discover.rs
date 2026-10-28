//! `openmhs discover`: find devices at addresses you name, without changing anything.
//!
//! Discovery proposes; it never registers a device or grants anything. Each protocol asks
//! only what has no effect:
//! - SCPI: `*IDN?` (IEEE 488.2);
//! - Modbus TCP: Read Device Identification (function 43/14), a read;
//! - HTTP: one `GET` of the URL given;
//! - MQTT: subscribe for a few seconds and list the topics seen; nothing is published.
//!
//! Addresses are given explicitly, at most 256 per run, a bounded number at a time. What
//! answers is listed with the command that starts a profile for it. Connecting a device is
//! still the operator's separate step (docs/connect-a-device.md). As for the adapters
//! (ADR-0016), plaintext HTTP and MQTT beyond loopback need `--allow-plaintext`.

use std::time::Duration;

use openmhs_scpi::link::{LineLink, StreamLink};
use serde::Serialize;

use crate::exit;

/// The most addresses one run asks.
pub const MAX_TARGETS: usize = 256;
const AT_ONCE: usize = 32;

#[derive(Debug, Serialize)]
pub struct Candidate {
    pub address: String,
    pub protocol: String,
    /// What the device said it is, exactly as it said it.
    pub identity: String,
    /// Start a profile for it; nothing is connected or registered until you do.
    pub next: String,
}

#[derive(Debug, Serialize)]
pub struct Discovery {
    pub asked: usize,
    pub candidates: Vec<Candidate>,
    /// Addresses that did not answer in time, refused the connection, or answered with
    /// something that is not an identity.
    pub silent: usize,
}

/// `host`, `host:port`, or an IPv4 range `a.b.c.d-e` (last octet), each with `port` unless
/// one is given.
pub fn expand(targets: &[String], port: u16) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for t in targets {
        let (host, p) = match t.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (
                h.to_owned(),
                p.parse::<u16>()
                    .map_err(|_| format!("'{t}': '{p}' is not a port"))?,
            ),
            _ => (t.clone(), port),
        };
        match host.rsplit_once('.').and_then(|(net, last)| {
            last.split_once('-')
                .map(|(lo, hi)| (net.to_owned(), lo.to_owned(), hi.to_owned()))
        }) {
            Some((net, lo, hi)) => {
                let (lo, hi): (u8, u8) = (
                    lo.parse().map_err(|_| format!("'{t}' is not a range"))?,
                    hi.parse().map_err(|_| format!("'{t}' is not a range"))?,
                );
                if lo > hi {
                    return Err(format!("'{t}': the range runs backwards"));
                }
                for n in lo..=hi {
                    out.push(format!("{net}.{n}:{p}"));
                }
            }
            None => out.push(format!("{host}:{p}")),
        }
        if out.len() > MAX_TARGETS {
            return Err(format!(
                "at most {MAX_TARGETS} addresses per run; name a smaller range"
            ));
        }
    }
    Ok(out)
}

/// A line that looks like an IEEE 488.2 identity: manufacturer, model, serial, firmware.
fn is_identity(line: &str) -> bool {
    line.split(',').count() >= 4 && line.len() <= 200 && !line.trim().is_empty()
}

pub async fn scpi(targets: Vec<String>, timeout: Duration) -> Discovery {
    let asked = targets.len();
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    let mut probes = tokio::task::JoinSet::new();
    for address in targets {
        let limit = limit.clone();
        probes.spawn(async move {
            let _permit = limit.acquire_owned().await;
            let link = StreamLink::tcp(address.clone());
            let answer = link.transact("*IDN?", timeout).await.ok()?;
            let identity = answer.trim().to_owned();
            is_identity(&identity).then_some((address, identity))
        });
    }
    let mut candidates = Vec::new();
    while let Some(done) = probes.join_next().await {
        if let Ok(Some((address, identity))) = done {
            let model = identity
                .split(',')
                .nth(1)
                .map(|m| {
                    m.trim()
                        .to_lowercase()
                        .replace(|c: char| !c.is_ascii_alphanumeric(), "-")
                })
                .unwrap_or_default();
            candidates.push(Candidate {
                next: format!("openmhs profile new scpi --id lab.{model}"),
                address,
                protocol: openmhs_scpi::PROTOCOL.to_owned(),
                identity,
            });
        }
    }
    candidates.sort_by(|a, b| a.address.cmp(&b.address));
    Discovery {
        asked,
        silent: asked - candidates.len(),
        candidates,
    }
}

pub fn print(d: &Discovery) {
    println!(
        "asked {} address(es) with read-only identification; nothing was changed or registered",
        d.asked
    );
    for c in &d.candidates {
        println!("  {:<22} {}  {}", c.address, c.protocol, c.identity);
        println!("  {:<22} next: {}", "", c.next);
    }
    if d.candidates.is_empty() {
        println!("  no instrument answered");
    }
    if d.silent > 0 && !d.candidates.is_empty() {
        println!("  {} did not answer", d.silent);
    }
}

pub async fn run(json: bool, targets: &[String], port: u16, timeout_ms: u64) -> u8 {
    let targets = match expand(targets, port) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    let d = scpi(targets, Duration::from_millis(timeout_ms)).await;
    if json {
        crate::output::json(&d);
    } else {
        print(&d);
    }
    exit::OK
}

// ------------------------------------------------------------------ Modbus TCP

/// Function 43 / MEI 14: Read Device Identification, basic objects (vendor, product code,
/// revision). A read; devices that do not implement it answer with an exception, which still
/// shows that something speaks Modbus there.
async fn modbus_identify(address: &str, unit: u8, timeout: Duration) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let work = async {
        let mut s = tokio::net::TcpStream::connect(address).await.ok()?;
        // MBAP: transaction 1, protocol 0, length 5, unit; PDU: 0x2B 0x0E 0x01 (basic) 0x00.
        let request = [0, 1, 0, 0, 0, 5, unit, 0x2B, 0x0E, 0x01, 0x00];
        s.write_all(&request).await.ok()?;
        let mut header = [0u8; 7];
        s.read_exact(&mut header).await.ok()?;
        if header[2..4] != [0, 0] {
            return None; // not Modbus TCP
        }
        let len = usize::from(u16::from_be_bytes([header[4], header[5]]));
        if !(2..=253).contains(&len) {
            return None;
        }
        let mut pdu = vec![0u8; len - 1];
        s.read_exact(&mut pdu).await.ok()?;
        match pdu.as_slice() {
            [0xAB, code, ..] => Some(format!(
                "a Modbus device that does not report its identity (exception {code})"
            )),
            [0x2B, 0x0E, _, _, _, _, count, rest @ ..] => {
                let mut fields = Vec::new();
                let mut at = rest;
                for _ in 0..*count {
                    let [_id, n, tail @ ..] = at else { break };
                    let n = usize::from(*n);
                    if tail.len() < n {
                        break;
                    }
                    fields.push(String::from_utf8_lossy(&tail[..n]).trim().to_owned());
                    at = &tail[n..];
                }
                (!fields.is_empty()).then(|| fields.join(","))
            }
            _ => None,
        }
    };
    tokio::time::timeout(timeout, work).await.ok().flatten()
}

pub async fn modbus(targets: Vec<String>, unit: u8, timeout: Duration) -> Discovery {
    gather(targets, move |address| async move {
        let identity = modbus_identify(&address, unit, timeout).await?;
        Some(Candidate {
            next: "openmhs profile new modbus-tcp --id lab.device".into(),
            protocol: "modbus-tcp".into(),
            address,
            identity,
        })
    })
    .await
}

// ------------------------------------------------------------------ HTTP

pub async fn http(urls: Vec<String>, timeout: Duration) -> Discovery {
    openmhs_profile::tls::install_provider();
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return gather(Vec::new(), |_| async { None }).await,
    };
    gather(urls, move |url| {
        let client = client.clone();
        async move {
            let r = client.get(&url).send().await.ok()?;
            let header = |name: &str| {
                r.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let identity = format!(
                "HTTP {}{}{}",
                r.status().as_u16(),
                header("server")
                    .map(|s| format!(", server {s}"))
                    .unwrap_or_default(),
                header("content-type")
                    .map(|c| format!(", {c}"))
                    .unwrap_or_default()
            );
            Some(Candidate {
                next: "openmhs profile new http --id lab.device".into(),
                protocol: "http".into(),
                address: url,
                identity,
            })
        }
    })
    .await
}

// ------------------------------------------------------------------ MQTT

/// Subscribe to `filter` on `broker` for `listen` and report the topics seen (at most 500).
/// Publishes nothing. Retained messages arrive at once, so `--for` a few seconds is enough
/// for nodes that keep their state retained.
pub async fn mqtt(broker: &str, filter: &str, listen: Duration) -> Discovery {
    use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
    let (host, port) = match broker.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.parse().unwrap_or(1883)),
        None => (broker.to_owned(), 1883),
    };
    let mut options = MqttOptions::new(
        format!("openmhs-discover-{}", std::process::id()),
        host.clone(),
        port,
    );
    options.set_keep_alive(Duration::from_secs(5));
    options.set_clean_session(true);
    let (client, mut events) = AsyncClient::new(options, 16);
    let mut topics: std::collections::BTreeMap<String, usize> = Default::default();
    let mut connected = false;
    let deadline = tokio::time::Instant::now() + listen;
    while let Ok(polled) = tokio::time::timeout_at(deadline, events.poll()).await {
        match polled {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                connected = true;
                if client.subscribe(filter, QoS::AtMostOnce).await.is_err() {
                    break;
                }
            }
            Ok(Event::Incoming(Packet::Publish(p))) if topics.len() < 500 => {
                *topics.entry(p.topic.clone()).or_default() += 1;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = client.disconnect().await;
    let candidates = topics
        .into_iter()
        .map(|(topic, n)| Candidate {
            address: format!("{host}:{port}/{topic}"),
            protocol: "mqtt".into(),
            identity: format!("{n} message(s) in {} s", listen.as_secs()),
            next: "openmhs profile new mqtt --id lab.device".into(),
        })
        .collect::<Vec<_>>();
    Discovery {
        asked: usize::from(connected),
        silent: usize::from(!connected),
        candidates,
    }
}

/// Probe each target, a bounded number at a time, and collect what answered.
async fn gather<F, Fut>(targets: Vec<String>, probe: F) -> Discovery
where
    F: Fn(String) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Option<Candidate>> + Send + 'static,
{
    let asked = targets.len();
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    let mut probes = tokio::task::JoinSet::new();
    for t in targets {
        let (limit, probe) = (limit.clone(), probe.clone());
        probes.spawn(async move {
            let _permit = limit.acquire_owned().await;
            probe(t).await
        });
    }
    let mut candidates = Vec::new();
    while let Some(done) = probes.join_next().await {
        if let Ok(Some(c)) = done {
            candidates.push(c);
        }
    }
    candidates.sort_by(|a, b| a.address.cmp(&b.address));
    Discovery {
        asked,
        silent: asked - candidates.len(),
        candidates,
    }
}

/// Plaintext beyond loopback is refused unless asked for, as for the adapters (ADR-0016).
pub fn check_plaintext(host: &str, plaintext: bool, allow: bool) -> Result<(), String> {
    if plaintext && !allow && !openmhs_profile::tls::is_loopback(host) {
        return Err(format!(
            "'{host}' is not loopback and the connection would be unencrypted; pass --allow-plaintext on a network you trust"
        ));
    }
    Ok(())
}

pub fn finish(json: bool, d: &Discovery) -> u8 {
    if json {
        crate::output::json(d);
    } else {
        print(d);
    }
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_expand_and_are_bounded() {
        assert_eq!(
            expand(&["10.0.0.5".into(), "10.0.0.9:5555".into()], 5025).unwrap(),
            ["10.0.0.5:5025", "10.0.0.9:5555"]
        );
        assert_eq!(expand(&["10.0.0.1-3".into()], 5025).unwrap().len(), 3);
        assert!(expand(&["10.0.0.3-1".into()], 5025).is_err());
        let wide: Vec<String> = (0..2).map(|n| format!("10.0.{n}.0-255")).collect();
        assert!(
            expand(&wide, 5025).is_err(),
            "more than 256 addresses is refused"
        );
    }

    #[test]
    fn only_identity_shaped_answers_count() {
        assert!(is_identity("ACME,PSU-1,42,1.0"));
        assert!(!is_identity("1"));
        assert!(!is_identity(""));
    }
}
