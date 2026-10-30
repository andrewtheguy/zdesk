//! The relay's side of the wire: it connects gateways to the Macs it allows, and
//! carries their media streams between a gateway's media connection and the
//! Mac's UDP ports.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::AsyncWriteExt as _;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};

use crate::{Frame, PORT_QUEUE, VERSION, read_line};

/// How long a new connection has to say what it is for, and the Mac to accept
/// the relay's connection.
const DEADLINE: Duration = Duration::from_secs(10);

/// The receive buffer of each port bound towards the Mac: a keyframe of its
/// screen video comes as one burst, as the gateway's own socket for it is sized.
const UDP_RECEIVE_BUFFER: usize = 4 * 1024 * 1024;

/// A network the relay may connect to: an address and how many of its leading
/// bits must match, written `192.168.160.0/24`, or an address alone for that
/// one address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    addr: IpAddr,
    prefix: u8,
}

impl std::str::FromStr for Network {
    type Err = anyhow::Error;

    fn from_str(written: &str) -> anyhow::Result<Self> {
        let (addr, prefix) = match written.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (written, None),
        };
        let addr: IpAddr = addr.parse().with_context(|| format!("{addr:?} is not an IP address"))?;
        let addr = addr.to_canonical();
        let bits = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse().with_context(|| format!("{prefix:?} is not a prefix length"))?,
            None => bits,
        };
        anyhow::ensure!(prefix <= bits, "{written:?} has a prefix longer than its address");
        Ok(Self { addr, prefix })
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

impl Network {
    fn contains(&self, ip: IpAddr) -> bool {
        let leading = |a: u128, b: u128, bits: u32| {
            let shift = bits - u32::from(self.prefix);
            shift >= bits || (a >> shift) == (b >> shift)
        };
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => leading(u32::from(net).into(), u32::from(ip).into(), 32),
            (IpAddr::V6(net), IpAddr::V6(ip)) => leading(u128::from(net), u128::from(ip), 128),
            _ => false,
        }
    }
}

/// Serve gateways on `listener`, connecting them only to addresses in `allow`.
pub async fn serve(listener: TcpListener, allow: Vec<Network>) -> anyhow::Result<()> {
    let relay = Arc::new(Relay { allow, connections: Mutex::default() });
    loop {
        let (stream, from) = listener.accept().await.context("accepting a connection")?;
        let relay = Arc::clone(&relay);
        tokio::spawn(async move {
            if let Err(e) = relay.handle(stream, from).await {
                log::warn!("{from}: {e:#}");
            }
        });
    }
}

struct Relay {
    allow: Vec<Network>,
    /// The Mac connections a media connection may join, by token.
    connections: Mutex<HashMap<String, Joinable>>,
}

/// What a media connection needs of the Mac connection it joins.
struct Joinable {
    /// The relay's address on the connection to the Mac, which the Mac sends its
    /// media to.
    near: IpAddr,
    mac: IpAddr,
    /// Turns true when the Mac connection ends, which ends its media connection.
    ended: watch::Receiver<bool>,
    joined: bool,
}

impl Relay {
    async fn handle(self: Arc<Self>, mut stream: TcpStream, from: SocketAddr) -> anyhow::Result<()> {
        let line = tokio::time::timeout(DEADLINE, read_line(&mut stream))
            .await
            .context("the connection said nothing")??;
        let request = line.strip_prefix(VERSION).and_then(|rest| rest.strip_prefix(' '));
        match request.and_then(|request| request.split_once(' ')) {
            Some(("connect", dest)) => self.connect(stream, from, dest).await,
            Some(("media", token)) => self.media(stream, token).await,
            _ => Err(refuse(stream, format!("{line:?} is not a request of {VERSION}")).await),
        }
    }

    async fn connect(self: Arc<Self>, mut client: TcpStream, from: SocketAddr, dest: &str) -> anyhow::Result<()> {
        let addresses = match tokio::net::lookup_host(dest).await {
            Ok(addresses) => addresses.collect::<Vec<_>>(),
            Err(e) => return Err(refuse(client, format!("cannot resolve {dest}: {e}")).await),
        };
        let Some(addr) = addresses.into_iter().find(|addr| self.allow.iter().any(|net| net.contains(addr.ip())))
        else {
            return Err(refuse(client, format!("{dest} is not an address this relay allows")).await);
        };
        let mut mac = match tokio::time::timeout(DEADLINE, TcpStream::connect(addr)).await {
            Ok(Ok(mac)) => mac,
            Ok(Err(e)) => return Err(refuse(client, format!("connecting to {addr}: {e}")).await),
            Err(_) => return Err(refuse(client, format!("connecting to {addr} timed out")).await),
        };
        let mut bytes = [0u8; 16];
        rand::fill(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let (ended, joinable) = watch::channel(false);
        let joinable = Joinable {
            near: mac.local_addr()?.ip().to_canonical(),
            mac: addr.ip().to_canonical(),
            ended: joinable,
            joined: false,
        };
        self.connections.lock().unwrap().insert(token.clone(), joinable);
        let forget = Forget { relay: &self, token: &token, ended };
        client.write_all(format!("ok {token}\n").as_bytes()).await?;
        log::info!("{from}: connected to {addr}");
        client.set_nodelay(true)?;
        mac.set_nodelay(true)?;
        let carried = tokio::io::copy_bidirectional(&mut client, &mut mac).await;
        drop(forget);
        match carried {
            Ok((up, down)) => log::info!("{from}: the connection to {addr} ended, {up} bytes to it and {down} from it"),
            Err(e) => log::info!("{from}: the connection to {addr} ended: {e}"),
        }
        Ok(())
    }

    async fn media(self: Arc<Self>, mut stream: TcpStream, token: &str) -> anyhow::Result<()> {
        let joined = match self.connections.lock().unwrap().get_mut(token) {
            Some(joinable) if !joinable.joined => {
                joinable.joined = true;
                Ok((joinable.near, joinable.mac, joinable.ended.clone()))
            }
            Some(_) => Err("that connection already has its media connection".to_owned()),
            None => Err(format!("no connection is named {token}")),
        };
        let (near, mac, mut ended) = match joined {
            Ok(joined) => joined,
            Err(reason) => return Err(refuse(stream, reason).await),
        };
        stream.write_all(b"ok\n").await?;
        stream.set_nodelay(true)?;
        log::info!("{mac}: media connection joined");
        let (mut reader, mut writer) = stream.into_split();
        let (out, mut outgoing) = mpsc::channel::<Vec<u8>>(PORT_QUEUE);
        let mut ports: HashMap<u16, (Arc<UdpSocket>, Arc<Counts>)> = HashMap::new();
        let mut pumps = tokio::task::JoinSet::new();
        let writing = async {
            while let Some(frame) = outgoing.recv().await {
                writer.write_all(&frame).await.context("sending to the gateway")?;
            }
            anyhow::Ok(())
        };
        let reading = async {
            loop {
                match Frame::read(&mut reader).await? {
                    None => return anyhow::Ok(()),
                    Some(Frame::Open { port }) => {
                        if ports.contains_key(&port) {
                            continue;
                        }
                        let socket = Arc::new(bind(near, mac, port)?);
                        log::info!("{mac}: relaying UDP {port}");
                        let counts = Arc::new(Counts::default());
                        pumps.spawn(pump(Arc::clone(&socket), port, out.clone(), Arc::clone(&counts)));
                        ports.insert(port, (socket, counts));
                    }
                    Some(Frame::Datagram { port, payload }) => {
                        if let Some((socket, counts)) = ports.get(&port) {
                            counts.to_mac.fetch_add(1, Ordering::Relaxed);
                            // A refused or reset port is the Mac not listening yet,
                            // as on the gateway's own socket.
                            let _ = socket.send(&payload).await;
                        }
                    }
                }
            }
        };
        let result = tokio::select! {
            result = reading => result,
            result = writing => result,
            _ = ended.wait_for(|ended| *ended) => Ok(()),
        };
        pumps.abort_all();
        for (port, (_, counts)) in &ports {
            log::info!(
                "{mac}: UDP {port} carried {} datagrams from the Mac and {} to it",
                counts.from_mac.load(Ordering::Relaxed),
                counts.to_mac.load(Ordering::Relaxed)
            );
        }
        result.context("the media connection failed")
    }
}

/// Takes a Mac connection's token back, and ends its media connection, when the
/// Mac connection ends.
struct Forget<'a> {
    relay: &'a Relay,
    token: &'a str,
    ended: watch::Sender<bool>,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        let _ = self.ended.send(true);
        self.relay.connections.lock().unwrap().remove(self.token);
    }
}

#[derive(Default)]
struct Counts {
    from_mac: AtomicU64,
    to_mac: AtomicU64,
}

/// Answer `reason` and close: the error that ends the connection.
async fn refuse(mut stream: TcpStream, reason: String) -> anyhow::Error {
    let _ = stream.write_all(format!("error {reason}\n").as_bytes()).await;
    anyhow::anyhow!(reason)
}

/// `port` on the relay's address towards the Mac, connected to the Mac's port
/// of the same number. With address and port reuse, as the gateway binds its
/// own: every Mac names the same numbers, so several sessions share them, and
/// each socket being connected is what hands it its own Mac's datagrams.
fn bind(near: IpAddr, mac: IpAddr, port: u16) -> anyhow::Result<UdpSocket> {
    let at = SocketAddr::new(near, port);
    let to = SocketAddr::new(mac, port);
    let socket = socket2::Socket::new(socket2::Domain::for_address(at), socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    if let Err(e) = socket.set_recv_buffer_size(UDP_RECEIVE_BUFFER) {
        log::warn!("UDP {at} keeps its default receive buffer, short of {UDP_RECEIVE_BUFFER} bytes: {e}");
    }
    socket.bind(&at.into()).with_context(|| format!("binding UDP {at}"))?;
    socket.connect(&to.into()).with_context(|| format!("connecting UDP {at} to {to}"))?;
    socket.set_nonblocking(true)?;
    Ok(UdpSocket::from_std(socket.into())?)
}

/// What the Mac sends to `port`, framed for the gateway.
async fn pump(socket: Arc<UdpSocket>, port: u16, out: mpsc::Sender<Vec<u8>>, counts: Arc<Counts>) {
    let mut datagram = vec![0u8; 65_536];
    loop {
        match socket.recv(&mut datagram).await {
            Ok(len) => {
                counts.from_mac.fetch_add(1, Ordering::Relaxed);
                let frame = Frame::Datagram { port, payload: datagram[..len].to_vec() }.encode();
                if out.send(frame).await.is_err() {
                    return;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset) => {}
            Err(e) => {
                log::warn!("UDP {port} failed: {e}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_network_holds_the_addresses_its_prefix_covers() {
        let net: Network = "192.168.160.0/24".parse().unwrap();
        assert!(net.contains("192.168.160.2".parse().unwrap()));
        assert!(net.contains("::ffff:192.168.160.2".parse().unwrap()));
        assert!(!net.contains("192.168.161.2".parse().unwrap()));
        assert!(!net.contains("fdb8::2".parse().unwrap()));
        let one: Network = "fdb8::2".parse().unwrap();
        assert!(one.contains("fdb8::2".parse().unwrap()));
        assert!(!one.contains("fdb8::3".parse().unwrap()));
        let all: Network = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("10.1.2.3".parse().unwrap()));
        assert!("10.0.0.0/33".parse::<Network>().is_err());
        assert!("mac.local".parse::<Network>().is_err());
    }

    /// A gateway reaches a Mac through the relay and trades datagrams with it.
    /// Linux alone, where all of 127/8 is the loopback's without an alias: the
    /// Mac at 127.0.0.2 and the relay reaching it from 127.0.0.1 have one port
    /// number each without sharing an address.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_gateway_reaches_a_mac_and_its_media_through_the_relay() {
        use tokio::io::AsyncReadExt as _;

        let mac_tcp = TcpListener::bind("127.0.0.2:0").await.unwrap();
        let mac_udp = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let port = mac_udp.local_addr().unwrap().port();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay = listener.local_addr().unwrap().to_string();
        tokio::spawn(serve(listener, vec!["127.0.0.2".parse().unwrap()]));

        let mut gateway = TcpStream::connect(&relay).await.unwrap();
        let refused = crate::open(&mut TcpStream::connect(&relay).await.unwrap(), "127.0.0.3:5900").await;
        assert!(refused.unwrap_err().to_string().contains("not an address this relay allows"));
        let dest = mac_tcp.local_addr().unwrap().to_string();
        let token = crate::open(&mut gateway, &dest).await.unwrap();
        let (mut mac, from) = mac_tcp.accept().await.unwrap();
        mac.write_all(b"RFB 003.889\n").await.unwrap();
        let mut banner = [0u8; 12];
        gateway.read_exact(&mut banner).await.unwrap();
        assert_eq!(&banner, b"RFB 003.889\n");

        let link = crate::MediaLink::connect(&relay, &token).await.unwrap();
        let opened = link.open(port);
        mac_udp.connect((from.ip(), port)).await.unwrap();
        let mut datagram = [0u8; 64];
        // The relay binds the port once its open arrives; until then the Mac's
        // datagrams are refused.
        let len = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let _ = mac_udp.send(b"picture").await;
                if let Ok(Ok(len)) = tokio::time::timeout(Duration::from_millis(50), opened.recv(&mut datagram)).await {
                    return len;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(&datagram[..len], b"picture");
        opened.send(b"report").await.unwrap();
        let len = tokio::time::timeout(Duration::from_secs(5), mac_udp.recv(&mut datagram)).await.unwrap().unwrap();
        assert_eq!(&datagram[..len], b"report");

        let again = crate::MediaLink::connect(&relay, &token).await;
        assert!(again.err().unwrap().to_string().contains("already has its media connection"));

        // The Mac's connection ending ends the media connection.
        drop(mac);
        drop(gateway);
        let ended = tokio::time::timeout(Duration::from_secs(5), opened.recv(&mut datagram)).await.unwrap();
        assert_eq!(ended.unwrap_err().kind(), std::io::ErrorKind::ConnectionAborted);
    }
}
