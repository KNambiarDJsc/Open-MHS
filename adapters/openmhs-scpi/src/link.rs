//! Line links: one request line out, one reply line back, over TCP or a serial port.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use openmhs_types::Source;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// How a transaction failed. The class decides whether the device might have acted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// No connection: the request was certainly not sent.
    Down(String),
    /// The connection broke after the request may have left.
    Lost(String),
    /// The request was sent and no reply came in time.
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
pub trait LineLink: Send + Sync {
    /// Send one line and wait up to `timeout` for one reply line (without terminator).
    async fn transact(&self, request: &str, timeout: Duration) -> Result<String, LinkError>;

    /// What readings through this link come from.
    fn source(&self) -> Source;
}

/// A byte stream a link can run over.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

type Opened = Pin<Box<dyn Future<Output = io::Result<Box<dyn Stream>>> + Send>>;
type Opener = Box<dyn Fn() -> Opened + Send + Sync>;

struct Conn {
    stream: BufReader<Box<dyn Stream>>,
    /// A reply may still be on its way from a request that timed out.
    stale: bool,
}

/// A line link over any stream, opened on first use and reopened after the connection
/// breaks. After a timeout the stream is kept, and input left over from the late reply
/// is discarded before the next request.
pub struct StreamLink {
    open: Opener,
    conn: tokio::sync::Mutex<Option<Conn>>,
    source: Source,
}

impl StreamLink {
    /// A link that opens its stream with `open`.
    pub fn new<F, Fut, S>(open: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = io::Result<S>> + Send + 'static,
        S: Stream + 'static,
    {
        Self {
            open: Box::new(move || {
                let fut = open();
                Box::pin(async move { fut.await.map(|s| Box::new(s) as Box<dyn Stream>) })
            }),
            conn: tokio::sync::Mutex::new(None),
            source: Source::Device,
        }
    }

    /// TCP to `addr` (`host:port`), e.g. port 5025, the usual SCPI socket port.
    pub fn tcp(addr: impl Into<String>) -> Self {
        let addr = addr.into();
        Self::new(move || tokio::net::TcpStream::connect(addr.clone()))
    }

    /// A serial port (`COM3`, `/dev/ttyUSB0`) at `baud`, 8 data bits, no parity, 1 stop bit.
    pub fn serial(port: impl Into<String>, baud: u32) -> Self {
        let port = port.into();
        Self::new(move || {
            let builder = tokio_serial::new(port.clone(), baud);
            async move { tokio_serial::SerialStream::open(&builder).map_err(io::Error::from) }
        })
    }

    /// Mark readings as coming from a simulator rather than hardware.
    #[must_use]
    pub fn simulated(mut self) -> Self {
        self.source = Source::Simulator;
        self
    }
}

/// Read and discard whatever is already waiting.
async fn discard_pending(stream: &mut BufReader<Box<dyn Stream>>) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        match tokio::time::timeout(Duration::from_millis(5), stream.read_line(&mut line)).await {
            Ok(Ok(0)) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed")),
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Ok(()),
        }
    }
}

#[async_trait]
impl LineLink for StreamLink {
    async fn transact(&self, request: &str, timeout: Duration) -> Result<String, LinkError> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_mut()
            && c.stale
        {
            match discard_pending(&mut c.stream).await {
                Ok(()) => c.stale = false,
                Err(_) => *guard = None,
            }
        }
        let conn = match guard.as_mut() {
            Some(c) => c,
            None => {
                let stream = (self.open)()
                    .await
                    .map_err(|e| LinkError::Down(e.to_string()))?;
                guard.insert(Conn {
                    stream: BufReader::new(stream),
                    stale: false,
                })
            }
        };

        let line = format!("{request}\n");
        let sent = async {
            conn.stream.get_mut().write_all(line.as_bytes()).await?;
            conn.stream.get_mut().flush().await
        };
        if let Err(e) = sent.await {
            *guard = None;
            return Err(LinkError::Lost(e.to_string()));
        }
        let mut reply = String::new();
        match tokio::time::timeout(timeout, conn.stream.read_line(&mut reply)).await {
            Err(_) => {
                conn.stale = true;
                Err(LinkError::Timeout)
            }
            Ok(Ok(0)) => {
                *guard = None;
                Err(LinkError::Lost("the device closed the connection".into()))
            }
            Ok(Err(e)) => {
                *guard = None;
                Err(LinkError::Lost(e.to_string()))
            }
            Ok(Ok(_)) => Ok(reply.trim_end_matches(['\r', '\n']).to_owned()),
        }
    }

    fn source(&self) -> Source {
        self.source
    }
}
