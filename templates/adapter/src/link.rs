//! The link to the device: one request line out, one reply line back.
//!
//! Many instruments speak a protocol like this (SCPI over TCP port 5025, for example).
//! If yours does not, replace [`TcpLink`] with what it needs (serial, Modbus, HTTP, a
//! vendor SDK) and keep the [`LinkError`] classes: the adapter's error mapping depends on
//! them.

use std::time::Duration;

use async_trait::async_trait;
use openmhs_types::Source;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// How a request failed. The distinction matters: it decides whether the gateway may
/// assume nothing happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// No connection: the request was certainly not sent.
    Down(String),
    /// The connection broke after the request may have left. The device may have acted.
    Lost(String),
    /// The request was sent and no reply came in time. The device may have acted.
    Timeout,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Down(m) => write!(f, "link down: {m}"),
            LinkError::Lost(m) => write!(f, "connection lost: {m}"),
            LinkError::Timeout => f.write_str("no reply in time"),
        }
    }
}

#[async_trait]
pub trait Link: Send + Sync {
    /// Send one request line and wait up to `timeout` for one reply line.
    async fn transact(&self, request: &str, timeout: Duration) -> Result<String, LinkError>;

    /// What readings through this link come from: `Device` for real hardware,
    /// `Simulator` for a fake.
    fn source(&self) -> Source;
}

type Connection = (BufReader<OwnedReadHalf>, OwnedWriteHalf);

/// A line protocol over TCP. Connects on first use and reconnects after any error, so a
/// device that comes back is picked up without restarting the gateway.
pub struct TcpLink {
    addr: String,
    source: Source,
    conn: tokio::sync::Mutex<Option<Connection>>,
}

impl TcpLink {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            source: Source::Device,
            conn: tokio::sync::Mutex::new(None),
        }
    }

    /// Mark readings as coming from a simulator, for a fake device behind the socket.
    #[must_use]
    pub fn simulated(mut self) -> Self {
        self.source = Source::Simulator;
        self
    }
}

#[async_trait]
impl Link for TcpLink {
    async fn transact(&self, request: &str, timeout: Duration) -> Result<String, LinkError> {
        // One request at a time: replies are matched to requests by order.
        let mut guard = self.conn.lock().await;
        if guard.is_none() {
            let stream = tokio::time::timeout(timeout, TcpStream::connect(&self.addr))
                .await
                .map_err(|_| LinkError::Down(format!("connecting to {} timed out", self.addr)))?
                .map_err(|e| LinkError::Down(format!("connecting to {}: {e}", self.addr)))?;
            let (read, write) = stream.into_split();
            *guard = Some((BufReader::new(read), write));
        }
        let Some((reader, writer)) = guard.as_mut() else {
            return Err(LinkError::Down("not connected".into()));
        };
        let result = async {
            writer
                .write_all(format!("{request}\n").as_bytes())
                .await
                .map_err(|e| LinkError::Lost(e.to_string()))?;
            let mut line = String::new();
            match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                Err(_) => Err(LinkError::Timeout),
                Ok(Err(e)) => Err(LinkError::Lost(e.to_string())),
                Ok(Ok(0)) => Err(LinkError::Lost("the device closed the connection".into())),
                Ok(Ok(_)) => Ok(line.trim_end().to_owned()),
            }
        }
        .await;
        if result.is_err() {
            // After an error the stream may hold a late reply; start fresh next time.
            *guard = None;
        }
        result
    }

    fn source(&self) -> Source {
        self.source
    }
}
