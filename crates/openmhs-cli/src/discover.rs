//! `openmhs discover`: find instruments at addresses you name, without changing anything.
//!
//! Discovery proposes; it never registers a device or grants anything. It only asks each
//! address for its identity with a query that has no effect (`*IDN?`, IEEE 488.2), at
//! addresses given explicitly, at most 256 per run, a bounded number at a time. What
//! answers is listed with the command that starts a profile for it. Connecting a device is
//! still the operator's separate step (docs/connect-a-device.md).

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
    pub protocol: &'static str,
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
                protocol: openmhs_scpi::PROTOCOL,
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
        "asked {} address(es) for their identity (*IDN?); nothing was changed or registered",
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
