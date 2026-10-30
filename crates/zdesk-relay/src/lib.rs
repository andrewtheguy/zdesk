//! A relay between a zDesk gateway and a Mac that the gateway cannot reach itself.
//!
//! The gateway opens TCP connections to the relay's one port and to nothing
//! else, so a gateway behind a firewall needs no port opened to it. Each
//! connection starts with one line naming what it is for, and the relay answers
//! with one line:
//!
//! - `zdesk-relay/1 connect <host:port>` asks the relay to connect to the Mac.
//!   It answers `ok <token>`, and from then on the connection carries the Mac's
//!   TCP bytes both ways.
//! - `zdesk-relay/1 media <token>` joins the connection the token names, for
//!   High Performance's media stream. It answers `ok`, and from then on the
//!   connection carries [frames](Frame): the gateway opens each UDP port the Mac
//!   named, the relay says when it has bound it, and the datagrams of each port
//!   pass both ways.
//!
//! Every other answer is `error <reason>`, after which the relay closes the
//! connection.
//!
//! The Mac sends its media from its own address to the address its TCP
//! connection comes from, at the same port numbers it names, so the relay binds
//! each opened port on the address it reaches the Mac from and connects it to
//! the Mac's. Nothing is authenticated: anyone who reaches the relay's port can
//! connect through it to an address it allows.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

pub mod server;

/// What every request line starts with: the wire's name and version.
const VERSION: &str = "zdesk-relay/1";

/// The longest request or answer line, its newline included.
const MAX_LINE: usize = 512;

/// The datagrams of one port held for its reader before newer ones are dropped,
/// as a full socket buffer drops them.
const PORT_QUEUE: usize = 4096;

/// One unit of a media connection: an opened port, or a datagram of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// The gateway asks the relay to bind `port` towards the Mac, and the relay
    /// answers the same once it has.
    Open { port: u16 },
    /// A datagram of `port`: to the Mac from the gateway, or from the Mac to it.
    Datagram { port: u16, payload: Vec<u8> },
}

const KIND_OPEN: u8 = 0;
const KIND_DATAGRAM: u8 = 1;

impl Frame {
    /// The frame as it is written: its kind, its port and its payload's length,
    /// both big-endian, then the payload.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, port, payload): (u8, u16, &[u8]) = match self {
            Frame::Open { port } => (KIND_OPEN, *port, &[]),
            Frame::Datagram { port, payload } => (KIND_DATAGRAM, *port, payload),
        };
        let len = u16::try_from(payload.len()).expect("a UDP datagram is under 64 KiB");
        let mut out = Vec::with_capacity(5 + payload.len());
        out.push(kind);
        out.extend_from_slice(&port.to_be_bytes());
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// The next frame on `reader`, or `None` where the connection ends between two.
    pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> anyhow::Result<Option<Frame>> {
        let mut head = [0u8; 5];
        match reader.read_exact(&mut head).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(anyhow::Error::new(e).context("reading a frame")),
        }
        let port = u16::from_be_bytes([head[1], head[2]]);
        let len = usize::from(u16::from_be_bytes([head[3], head[4]]));
        let mut payload = vec![0u8; len];
        reader.read_exact(&mut payload).await.context("reading a frame's payload")?;
        match head[0] {
            KIND_OPEN => Ok(Some(Frame::Open { port })),
            KIND_DATAGRAM => Ok(Some(Frame::Datagram { port, payload })),
            kind => anyhow::bail!("unknown frame kind {kind}"),
        }
    }
}

/// One line off `stream`, read a byte at a time so that nothing after it is
/// taken: what follows an answer is already the Mac's.
async fn read_line<S: AsyncRead + Unpin>(stream: &mut S) -> anyhow::Result<String> {
    let mut line = Vec::new();
    loop {
        let byte = stream.read_u8().await.context("the line ended early")?;
        if byte == b'\n' {
            break;
        }
        anyhow::ensure!(line.len() < MAX_LINE, "a line longer than {MAX_LINE} bytes");
        line.push(byte);
    }
    String::from_utf8(line).context("a line that is not UTF-8")
}

/// The relay's answer to a request: what follows `ok`, or its error.
async fn read_answer<S: AsyncRead + Unpin>(stream: &mut S) -> anyhow::Result<String> {
    let line = read_line(stream).await.context("reading the relay's answer")?;
    if let Some(rest) = line.strip_prefix("ok") {
        return Ok(rest.trim_start().to_owned());
    }
    match line.strip_prefix("error ") {
        Some(reason) => anyhow::bail!("the relay refused: {reason}"),
        None => anyhow::bail!("the relay answered {line:?}, which is not this wire's"),
    }
}

/// Ask the relay at the far end of `stream` to connect to `dest`, a `host:port`.
/// On success the stream carries `dest`'s bytes, and the token returned names
/// the connection to [`MediaLink::connect`].
pub async fn open<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, dest: &str) -> anyhow::Result<String> {
    stream
        .write_all(format!("{VERSION} connect {dest}\n").as_bytes())
        .await
        .context("asking the relay to connect")?;
    let token = read_answer(stream).await?;
    anyhow::ensure!(!token.is_empty(), "the relay connected without naming the connection");
    Ok(token)
}

/// The gateway's half of a media connection: the ports it has opened, each
/// read and written as a connected UDP socket would be.
pub struct MediaLink {
    out: mpsc::UnboundedSender<Vec<u8>>,
    /// Each opened port's queue, kept across a port being opened again so that
    /// a reader taking over a port never sees it end.
    ports: Arc<Mutex<HashMap<u16, Queue>>>,
    /// The ports the relay has said it bound.
    opened: watch::Receiver<HashSet<u16>>,
}

#[derive(Clone)]
struct Queue {
    tx: mpsc::Sender<Vec<u8>>,
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>>,
}

impl MediaLink {
    /// Join the connection `token` names, on the relay at `relay`.
    pub async fn connect(relay: &str, token: &str) -> anyhow::Result<Self> {
        let mut stream = TcpStream::connect(relay)
            .await
            .with_context(|| format!("connecting to the relay at {relay} for the media stream"))?;
        stream.set_nodelay(true)?;
        stream
            .write_all(format!("{VERSION} media {token}\n").as_bytes())
            .await
            .context("asking the relay for the media stream")?;
        read_answer(&mut stream).await?;
        let (mut reader, mut writer) = stream.into_split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            while let Some(frame) = outgoing.recv().await {
                if let Err(e) = writer.write_all(&frame).await {
                    log::warn!("relay: the media connection failed while sending: {e}");
                    return;
                }
            }
        });
        let ports: Arc<Mutex<HashMap<u16, Queue>>> = Arc::default();
        let incoming = Arc::clone(&ports);
        let (bound, opened) = watch::channel(HashSet::new());
        tokio::spawn(async move {
            loop {
                match Frame::read(&mut reader).await {
                    Ok(Some(Frame::Datagram { port, payload })) => {
                        let tx = incoming.lock().unwrap().get(&port).map(|queue| queue.tx.clone());
                        // A full queue drops the datagram, as a full socket buffer would.
                        if let Some(tx) = tx {
                            let _ = tx.try_send(payload);
                        }
                    }
                    Ok(Some(Frame::Open { port })) => {
                        bound.send_modify(|bound| {
                            bound.insert(port);
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("relay: the media connection failed: {e:#}");
                        break;
                    }
                }
            }
            // Every port's reader then hears the connection close, and so does
            // anything waiting for a port to be bound, as `bound` goes.
            incoming.lock().unwrap().clear();
        });
        Ok(Self { out, ports, opened })
    }

    /// Resolves once the relay has bound every one of `ports`, which the Mac must
    /// not be asked to send to before: what it sends to a port not yet bound is
    /// lost. Fails if the media connection closes first.
    pub fn opened(&self, ports: &[u16]) -> impl Future<Output = std::io::Result<()>> + Send + use<> {
        let mut opened = self.opened.clone();
        let ports = ports.to_vec();
        async move {
            opened
                .wait_for(|bound| ports.iter().all(|port| bound.contains(port)))
                .await
                .map(|_| ())
                .map_err(|_| closed())
        }
    }

    /// Open `port`: the relay binds it towards the Mac, and what the Mac sends
    /// to it arrives here.
    pub fn open(&self, port: u16) -> Port {
        let queue = self
            .ports
            .lock()
            .unwrap()
            .entry(port)
            .or_insert_with(|| {
                let (tx, rx) = mpsc::channel(PORT_QUEUE);
                Queue { tx, rx: Arc::new(tokio::sync::Mutex::new(rx)) }
            })
            .clone();
        let _ = self.out.send(Frame::Open { port }.encode());
        Port { port, out: self.out.clone(), rx: queue.rx }
    }
}

/// One port of a [`MediaLink`], read and written as a UDP socket connected to
/// the Mac's port of the same number.
pub struct Port {
    port: u16,
    out: mpsc::UnboundedSender<Vec<u8>>,
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>>,
}

impl Port {
    /// Send `datagram` to the Mac's port.
    pub async fn send(&self, datagram: &[u8]) -> std::io::Result<usize> {
        let frame = Frame::Datagram { port: self.port, payload: datagram.to_vec() }.encode();
        self.out.send(frame).map_err(|_| closed())?;
        Ok(datagram.len())
    }

    /// The next datagram from the Mac's port, cut to `buffer` as a socket's read
    /// would cut it.
    pub async fn recv(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let datagram = self.rx.lock().await.recv().await.ok_or_else(closed)?;
        let len = datagram.len().min(buffer.len());
        buffer[..len].copy_from_slice(&datagram[..len]);
        Ok(len)
    }
}

fn closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "the relay's media connection closed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_read_back_as_written() {
        let frames = [
            Frame::Open { port: 5901 },
            Frame::Datagram { port: 5900, payload: vec![1, 2, 3] },
            Frame::Datagram { port: 65535, payload: vec![] },
        ];
        let written: Vec<u8> = frames.iter().flat_map(Frame::encode).collect();
        let mut reader = &written[..];
        for frame in &frames {
            assert_eq!(Frame::read(&mut reader).await.unwrap().as_ref(), Some(frame));
        }
        assert_eq!(Frame::read(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_unknown_frame_kind_is_refused() {
        let mut reader = &[9u8, 0, 1, 0, 0][..];
        assert!(Frame::read(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn an_answer_leaves_what_follows_it_unread() {
        let mut stream = &b"ok abc\nRFB 003.889\n"[..];
        assert_eq!(read_answer(&mut stream).await.unwrap(), "abc");
        assert_eq!(stream, b"RFB 003.889\n");
    }

    #[tokio::test]
    async fn an_error_answer_carries_its_reason() {
        let mut stream = &b"error 10.0.0.9 is not allowed\n"[..];
        let e = read_answer(&mut stream).await.unwrap_err();
        assert_eq!(e.to_string(), "the relay refused: 10.0.0.9 is not allowed");
    }
}
