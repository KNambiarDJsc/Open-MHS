//! Line links: one request line out, one reply line back, over TCP or a serial port.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
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
type Opener = Arc<dyn Fn() -> Opened + Send + Sync>;

struct Conn {
    stream: BufReader<Box<dyn Stream>>,
    /// Replies may still be on their way from requests that timed out.
    stale: bool,
    /// How many of those late replies answer the sync query, so that a resync does not
    /// stop at one of them while the answer to its own query is still to come.
    late_syncs: usize,
}

#[derive(Default)]
struct State {
    conn: Option<Conn>,
    /// The instrument's answer to the sync query, learnt the first time it is asked.
    identity: Option<String>,
}

/// A line link over any stream, opened on first use and reopened after the connection
/// breaks.
///
/// Every exchange runs to completion, or to its own timeout, even if the caller stops
/// waiting (a cancelled or timed-out command): a half-written line, or a reply left
/// unread, would otherwise be taken as the answer to the next request.
///
/// After a request times out its reply may still arrive, possibly long after, because an
/// instrument finishes a slow command before it reads the next one. Before anything else
/// is sent, the link sends the sync query (the profile's `identify`, e.g. `*IDN?`) and
/// discards every line until that query's known answer: instruments answer in order, so
/// whatever comes before it belongs to earlier requests. If the answer does not come in
/// time, the instrument is still busy and the new request is **not sent** (`Down`).
pub struct StreamLink {
    open: Opener,
    state: Arc<tokio::sync::Mutex<State>>,
    sync: Option<String>,
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
            open: Arc::new(move || {
                let fut = open();
                Box::pin(async move { fut.await.map(|s| Box::new(s) as Box<dyn Stream>) })
            }),
            state: Arc::new(tokio::sync::Mutex::new(State::default())),
            sync: None,
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

    /// Resynchronize after a timeout with `query`: a request that changes nothing and
    /// always gets the same answer (`*IDN?`). Without one, the link only discards what has
    /// already arrived, so a reply that comes later still can be taken for the next one.
    #[must_use]
    pub fn synced_by(mut self, query: impl Into<String>) -> Self {
        self.sync = Some(query.into());
        self
    }

    /// Mark readings as coming from a simulator rather than hardware.
    #[must_use]
    pub fn simulated(mut self) -> Self {
        self.source = Source::Simulator;
        self
    }
}

async fn write_line(conn: &mut Conn, line: &str) -> io::Result<()> {
    let out = conn.stream.get_mut();
    out.write_all(format!("{line}\n").as_bytes()).await?;
    out.flush().await
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

/// Why a resync did not finish.
enum Unsynced {
    /// The instrument is still busy: the connection stays, still stale.
    Busy,
    /// The connection broke.
    Lost,
}

/// Send the sync query and discard input until its answer, counting earlier sync queries
/// whose answers are still to come.
async fn resync(
    conn: &mut Conn,
    query: &str,
    identity: &str,
    timeout: Duration,
) -> Result<(), Unsynced> {
    write_line(conn, query).await.map_err(|_| Unsynced::Lost)?;
    conn.late_syncs += 1;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut line = String::new();
    while conn.late_syncs > 0 {
        line.clear();
        match tokio::time::timeout_at(deadline, conn.stream.read_line(&mut line)).await {
            Err(_) => return Err(Unsynced::Busy),
            Ok(Ok(0) | Err(_)) => return Err(Unsynced::Lost),
            Ok(Ok(_)) => {
                if line.trim_end_matches(['\r', '\n']) == identity {
                    conn.late_syncs -= 1;
                }
            }
        }
    }
    Ok(())
}

/// One exchange: resynchronize if an earlier one timed out, send `request`, read one
/// reply line.
async fn exchange(
    state: &tokio::sync::Mutex<State>,
    open: &Opener,
    sync: Option<&str>,
    request: &str,
    timeout: Duration,
) -> Result<String, LinkError> {
    let mut guard = state.lock().await;
    let st = &mut *guard;
    if let Some(conn) = st.conn.as_mut().filter(|c| c.stale) {
        match (sync, st.identity.as_deref()) {
            (Some(query), Some(identity)) => match resync(conn, query, identity, timeout).await {
                Ok(()) => conn.stale = false,
                Err(Unsynced::Busy) => {
                    return Err(LinkError::Down(format!(
                        "the instrument has not caught up with an earlier request that timed out; '{request}' was not sent"
                    )));
                }
                Err(Unsynced::Lost) => st.conn = None,
            },
            // Nothing to resynchronize with: drop what has already arrived, and hope.
            _ => match discard_pending(&mut conn.stream).await {
                Ok(()) => conn.stale = false,
                Err(_) => st.conn = None,
            },
        }
    }
    let conn = match st.conn.as_mut() {
        Some(c) => c,
        None => {
            let stream = open().await.map_err(|e| LinkError::Down(e.to_string()))?;
            st.conn.insert(Conn {
                stream: BufReader::new(stream),
                stale: false,
                late_syncs: 0,
            })
        }
    };

    if let Err(e) = write_line(conn, request).await {
        st.conn = None;
        return Err(LinkError::Lost(e.to_string()));
    }
    let mut reply = String::new();
    match tokio::time::timeout(timeout, conn.stream.read_line(&mut reply)).await {
        Err(_) => {
            conn.stale = true;
            if sync == Some(request) {
                conn.late_syncs += 1;
            }
            Err(LinkError::Timeout)
        }
        Ok(Ok(0)) => {
            st.conn = None;
            Err(LinkError::Lost("the device closed the connection".into()))
        }
        Ok(Err(e)) => {
            st.conn = None;
            Err(LinkError::Lost(e.to_string()))
        }
        Ok(Ok(_)) => {
            let reply = reply.trim_end_matches(['\r', '\n']).to_owned();
            if sync == Some(request) && st.identity.is_none() {
                st.identity = Some(reply.clone());
            }
            Ok(reply)
        }
    }
}

#[async_trait]
impl LineLink for StreamLink {
    async fn transact(&self, request: &str, timeout: Duration) -> Result<String, LinkError> {
        let (state, open, sync) = (self.state.clone(), self.open.clone(), self.sync.clone());
        let request = request.to_owned();
        // Detached, so that a caller who gives up cannot cut the exchange in half.
        tokio::spawn(
            async move { exchange(&state, &open, sync.as_deref(), &request, timeout).await },
        )
        .await
        .unwrap_or_else(|e| Err(LinkError::Lost(format!("link task failed: {e}"))))
    }

    fn source(&self) -> Source {
        self.source
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const IDN: &str = "ACME,PSU-1,42,1.0";

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A fake instrument that works like a real one: it reads a line, takes as long as
    /// `script` says, answers, and only then reads the next line. Every line it receives
    /// is recorded. It is one stream that cannot be reopened, like a serial port.
    fn instrument(
        script: impl Fn(&str) -> (u64, Option<String>) + Send + 'static,
    ) -> (StreamLink, Arc<StdMutex<Vec<String>>>) {
        let (ours, theirs) = tokio::io::duplex(4096);
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let mut s = BufReader::new(theirs);
            let mut line = String::new();
            loop {
                line.clear();
                if s.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let request = line.trim_end().to_owned();
                log.lock().unwrap().push(request.clone());
                let (delay, reply) = script(&request);
                tokio::time::sleep(ms(delay)).await;
                if let Some(r) = reply
                    && s.get_mut()
                        .write_all(format!("{r}\n").as_bytes())
                        .await
                        .is_err()
                {
                    return;
                }
            }
        });
        let port = Arc::new(StdMutex::new(Some(ours)));
        let link = StreamLink::new(move || {
            let taken = port.lock().unwrap().take();
            async move { taken.ok_or_else(|| io::Error::other("the port is already open")) }
        })
        .synced_by("*IDN?");
        (link, seen)
    }

    /// A power supply whose voltage change takes 300 ms to finish.
    fn psu(request: &str) -> (u64, Option<String>) {
        match request {
            "*IDN?" => (0, Some(IDN.into())),
            "SOUR:VOLT 5;*OPC?" => (300, Some("1".into())),
            "SOUR:VOLT?" => (0, Some("5.000".into())),
            "SLOW;*OPC?" => (1_500, Some("1".into())),
            _ => (0, Some("0".into())),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_late_reply_is_never_taken_for_the_next_answer() {
        let (link, _) = instrument(psu);
        assert_eq!(link.transact("*IDN?", ms(500)).await.unwrap(), IDN);
        assert_eq!(
            link.transact("SOUR:VOLT 5;*OPC?", ms(100)).await,
            Err(LinkError::Timeout)
        );
        // The "1" for the voltage change arrives after this query is sent. Read as the
        // answer, it would be a voltage of 1.
        assert_eq!(
            link.transact("SOUR:VOLT?", ms(1_000)).await.unwrap(),
            "5.000"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_caller_that_gives_up_does_not_cut_the_exchange_in_half() {
        let (link, _) = instrument(psu);
        link.transact("*IDN?", ms(500)).await.unwrap();
        // The caller stops waiting before the link's own timeout: a command deadline, a
        // cancelled task. The exchange still finishes and consumes its reply.
        assert!(
            tokio::time::timeout(ms(50), link.transact("SOUR:VOLT 5;*OPC?", ms(1_000)))
                .await
                .is_err()
        );
        assert_eq!(
            link.transact("SOUR:VOLT?", ms(1_000)).await.unwrap(),
            "5.000"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_busy_instrument_is_sent_nothing_new_until_it_catches_up() {
        let (link, seen) = instrument(psu);
        link.transact("*IDN?", ms(500)).await.unwrap();
        assert_eq!(
            link.transact("SLOW;*OPC?", ms(100)).await,
            Err(LinkError::Timeout)
        );
        let r = link.transact("OUTP ON;*OPC?", ms(200)).await;
        assert!(
            matches!(&r, Err(LinkError::Down(m)) if m.contains("was not sent")),
            "{r:?}"
        );
        assert!(
            !seen.lock().unwrap().iter().any(|l| l.starts_with("OUTP")),
            "{:?}",
            seen.lock().unwrap()
        );
        // Once it has caught up, the link is in step again.
        tokio::time::sleep(ms(1_500)).await;
        assert_eq!(
            link.transact("SOUR:VOLT?", ms(1_000)).await.unwrap(),
            "5.000"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_late_answer_to_the_sync_query_does_not_end_the_resync_early() {
        // The second *IDN? is slow (a health check that timed out); the others are not.
        let asked = AtomicUsize::new(0);
        let (link, _) = instrument(move |request| {
            if request == "*IDN?" && asked.fetch_add(1, Ordering::SeqCst) == 1 {
                return (300, Some(IDN.into()));
            }
            psu(request)
        });
        link.transact("*IDN?", ms(500)).await.unwrap();
        assert_eq!(
            link.transact("*IDN?", ms(100)).await,
            Err(LinkError::Timeout)
        );
        // Two identity lines are now due before the voltage: the late one and the resync's.
        assert_eq!(
            link.transact("SOUR:VOLT?", ms(1_000)).await.unwrap(),
            "5.000"
        );
    }
}
