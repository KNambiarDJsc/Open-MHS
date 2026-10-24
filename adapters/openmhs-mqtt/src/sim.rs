//! A simulated MQTT device, and the broker it publishes through.
//!
//! A small MQTT 3.1.1 broker written for the purpose (QoS 0 and 1, retained messages,
//! `+` and `#` filters) on rumqttc's packet codec, with the device built in: it publishes
//! its state topics, retained and timestamped, every 250 ms and after every command,
//! carries out messages on its command topics and acknowledges them. It misbehaves on
//! demand: refusals are acknowledged with `ok: false`, garbled acknowledgements are not
//! JSON, a silent device sends none, and a dead link drops every client.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use openmhs_profile::template::{self, put};
use openmhs_profile::{Outcome, VirtualDevice, codec};
use openmhs_types::{Clock, SystemClock};
use rumqttc::{
    ConnAck, ConnectReturnCode, Packet, PubAck, Publish, QoS, SubAck, SubscribeReasonCode, UnsubAck,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::Profile;

const MAX_PACKET: usize = 64 * 1024;
/// How often the device publishes its state.
const TELEMETRY: Duration = Duration::from_millis(250);
/// How often a connection checks whether the link has gone down.
const LINK_CHECK: Duration = Duration::from_millis(20);

pub struct MqttSim {
    profile: Profile,
    device: Arc<VirtualDevice>,
    broker: Mutex<Broker>,
}

#[derive(Default)]
struct Broker {
    next_id: u64,
    clients: BTreeMap<u64, Client>,
    retained: BTreeMap<String, Bytes>,
}

struct Client {
    filters: Vec<String>,
    tx: mpsc::UnboundedSender<Packet>,
}

/// Aborts a task when dropped, so the device stops publishing with the server.
struct Stop(JoinHandle<()>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Whether topic filter `filter` matches `topic`.
fn matches(filter: &str, topic: &str) -> bool {
    let mut levels = topic.split('/');
    for f in filter.split('/') {
        match (f, levels.next()) {
            ("#", _) => return true,
            ("+", Some(_)) => {}
            (f, Some(t)) if f == t => {}
            _ => return false,
        }
    }
    levels.next().is_none()
}

impl MqttSim {
    pub fn new(profile: Profile) -> Arc<Self> {
        let device = VirtualDevice::new(profile.descriptor.clone(), &profile.simulator.initial);
        Arc::new(Self {
            profile,
            device,
            broker: Mutex::new(Broker::default()),
        })
    }

    /// The device behind the protocol: its state, and the conformance fixture.
    pub fn device(&self) -> Arc<VirtualDevice> {
        self.device.clone()
    }

    fn broker(&self) -> MutexGuard<'_, Broker> {
        self.broker.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn telemetry(self: &Arc<Self>) -> Stop {
        let sim = self.clone();
        Stop(tokio::spawn(async move {
            loop {
                if !sim.device.link_down() {
                    sim.publish_state();
                }
                tokio::time::sleep(TELEMETRY).await;
            }
        }))
    }

    /// Serve on `listener` until the task is dropped.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        let _telemetry = self.telemetry();
        while let Ok((stream, _)) = listener.accept().await {
            if self.device.link_down() {
                continue;
            }
            let sim = self.clone();
            tokio::spawn(async move { sim.connection(stream).await });
        }
    }

    /// Serve MQTT over TLS on `listener` with `config` (for the TLS tests). A client
    /// whose TLS handshake fails is dropped.
    pub async fn serve_tls(
        self: Arc<Self>,
        listener: TcpListener,
        config: Arc<rustls::ServerConfig>,
    ) {
        let _telemetry = self.telemetry();
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
        let (tx, mut rx) = mpsc::unbounded_channel();
        let id = {
            let mut b = self.broker();
            b.next_id += 1;
            let id = b.next_id;
            b.clients.insert(
                id,
                Client {
                    filters: Vec::new(),
                    tx: tx.clone(),
                },
            );
            id
        };
        let (mut rd, mut wr) = tokio::io::split(stream);
        let mut buf = BytesMut::with_capacity(4096);
        let mut out = BytesMut::new();
        let mut link = tokio::time::interval(LINK_CHECK);
        'conn: loop {
            tokio::select! {
                n = rd.read_buf(&mut buf) => {
                    if !matches!(n, Ok(n) if n > 0) || self.device.link_down() {
                        break;
                    }
                    loop {
                        match Packet::read(&mut buf, MAX_PACKET) {
                            Ok(p) => {
                                if !self.handle(id, &tx, p) {
                                    break 'conn;
                                }
                            }
                            Err(rumqttc::mqttbytes::Error::InsufficientBytes(_)) => break,
                            Err(_) => break 'conn,
                        }
                    }
                }
                p = rx.recv() => {
                    let Some(p) = p else { break };
                    out.clear();
                    if p.write(&mut out, MAX_PACKET).is_err() || wr.write_all(&out).await.is_err() {
                        break;
                    }
                }
                _ = link.tick() => {
                    if self.device.link_down() {
                        break;
                    }
                }
            }
        }
        self.broker().clients.remove(&id);
    }

    /// Answer one packet from client `id`. `false` ends the connection.
    fn handle(&self, id: u64, tx: &mpsc::UnboundedSender<Packet>, packet: Packet) -> bool {
        match packet {
            Packet::Connect(_) => {
                let _ = tx.send(Packet::ConnAck(ConnAck::new(
                    ConnectReturnCode::Success,
                    false,
                )));
            }
            Packet::Subscribe(s) => {
                let mut b = self.broker();
                let codes = s
                    .filters
                    .iter()
                    .map(|_| SubscribeReasonCode::Success(QoS::AtMostOnce))
                    .collect();
                let _ = tx.send(Packet::SubAck(SubAck::new(s.pkid, codes)));
                for f in &s.filters {
                    for (topic, payload) in b.retained.iter().filter(|(t, _)| matches(&f.path, t)) {
                        let mut p = Publish::new(topic.clone(), QoS::AtMostOnce, payload.to_vec());
                        p.retain = true;
                        let _ = tx.send(Packet::Publish(p));
                    }
                }
                if let Some(c) = b.clients.get_mut(&id) {
                    for f in s.filters {
                        if !c.filters.contains(&f.path) {
                            c.filters.push(f.path);
                        }
                    }
                }
            }
            Packet::Unsubscribe(u) => {
                if let Some(c) = self.broker().clients.get_mut(&id) {
                    c.filters.retain(|f| !u.topics.contains(f));
                }
                let _ = tx.send(Packet::UnsubAck(UnsubAck { pkid: u.pkid }));
            }
            Packet::Publish(p) => {
                match p.qos {
                    QoS::AtMostOnce => {}
                    QoS::AtLeastOnce => {
                        let _ = tx.send(Packet::PubAck(PubAck::new(p.pkid)));
                    }
                    QoS::ExactlyOnce => return false, // not needed by any device here
                }
                self.route(&p.topic, p.payload.clone(), p.retain);
                self.command(&p.topic, &p.payload);
            }
            Packet::PingReq => {
                let _ = tx.send(Packet::PingResp);
            }
            Packet::Disconnect => return false,
            _ => {}
        }
        true
    }

    /// Deliver a message to every subscriber, and keep it if it is retained.
    fn route(&self, topic: &str, payload: Bytes, retain: bool) {
        let mut b = self.broker();
        if retain {
            if payload.is_empty() {
                b.retained.remove(topic);
            } else {
                b.retained.insert(topic.to_owned(), payload.clone());
            }
        }
        for c in b
            .clients
            .values()
            .filter(|c| c.filters.iter().any(|f| matches(f, topic)))
        {
            let _ = c.tx.send(Packet::Publish(Publish::new(
                topic,
                QoS::AtMostOnce,
                payload.to_vec(),
            )));
        }
    }

    /// Publish every state topic: the properties mapped there, and the time.
    fn publish_state(&self) {
        let mut docs: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        for (name, topics) in &self.profile.mapping {
            let Some(rule) = &topics.state else { continue };
            let doc = docs
                .entry(&rule.topic)
                .or_insert_with(|| serde_json::json!({}));
            if let Some(v) = self.device.read(name) {
                put(doc, &rule.pointer, codec::to_json(&v));
            }
            if let Some(t) = &rule.timestamp_pointer {
                put(doc, t, serde_json::json!(SystemClock.now().to_rfc3339()));
            }
        }
        for (topic, doc) in docs {
            self.route(topic, Bytes::from(doc.to_string()), true);
        }
    }

    fn ack(&self, id: &serde_json::Value, result: Result<(), String>) {
        let rule = &self.profile.connection.ack;
        let mut doc = serde_json::json!({});
        put(&mut doc, &rule.id_pointer, id.clone());
        put(
            &mut doc,
            &rule.ok_pointer,
            serde_json::json!(result.is_ok()),
        );
        if let (Err(reason), Some(p)) = (result, &rule.error_pointer) {
            put(&mut doc, p, serde_json::json!(reason));
        }
        self.route(&rule.topic, Bytes::from(doc.to_string()), false);
    }

    /// Carry out a message on a command topic, if it is one.
    fn command(&self, topic: &str, payload: &[u8]) {
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) else {
            return;
        };
        for (name, topics) in &self.profile.mapping {
            let (Some(rule), Some(cap)) =
                (&topics.command, self.profile.descriptor.capability(name))
            else {
                continue;
            };
            if rule.topic != topic {
                continue;
            }
            let Ok(slots) = template::json_slots(&rule.payload) else {
                continue;
            };
            if !slots.iter().all(|(_, p)| json.pointer(p).is_some()) {
                continue; // another capability shares this topic
            }
            let mut id = serde_json::Value::Null;
            let mut value = None;
            let mut params = BTreeMap::new();
            for (slot, pointer) in &slots {
                let Some(raw) = json.pointer(pointer) else {
                    continue;
                };
                if slot == "command_id" {
                    id = raw.clone();
                    continue;
                }
                let spec = if slot == "value" {
                    cap.value.as_ref()
                } else {
                    cap.params.as_ref().and_then(|p| p.get(slot))
                };
                let Some(spec) = spec else { continue };
                match codec::from_json(spec, raw) {
                    Ok(v) if slot == "value" => value = Some(v),
                    Ok(v) => {
                        params.insert(slot.clone(), v);
                    }
                    Err(e) => return self.ack(&id, Err(format!("{slot}: {e}"))),
                }
            }
            let outcome = self.device.command(name, value, &params);
            self.publish_state();
            match outcome {
                Outcome::Applied => self.ack(&id, Ok(())),
                Outcome::Refused(reason) => self.ack(&id, Err(reason)),
                Outcome::Silent => {}
                Outcome::Garbled => {
                    self.route(
                        &self.profile.connection.ack.topic,
                        Bytes::from_static(b"\x01#?K"),
                        false,
                    );
                }
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn topic_filters_match_like_a_broker() {
        assert!(matches("a/b/c", "a/b/c"));
        assert!(matches("a/+/c", "a/b/c"));
        assert!(matches("a/#", "a/b/c"));
        assert!(matches("#", "a"));
        assert!(!matches("a/+", "a/b/c"));
        assert!(!matches("a/b/c", "a/b"));
        assert!(!matches("a/b", "a/b/c"));
    }
}
