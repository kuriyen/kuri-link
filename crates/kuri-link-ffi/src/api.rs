//! Everything `flutter_rust_bridge_codegen` turns into Dart. Public items here
//! are the Dart package's API; keep them plain.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

use flutter_rust_bridge::frb;
use kuri_link::{CallError, Node, PeerId, RelayUrl, Relays, Ticket, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

/// How to reach a peer: its key, plus any addresses known out of band.
#[derive(Debug, Clone)]
pub struct PeerAddr {
    /// The peer's public key: 64 hex digits, `PeerId`'s text form.
    pub id: String,
    /// Direct socket addresses, e.g. `"192.0.2.40:41234"`.
    pub addrs: Vec<String>,
    /// Relays the peer is reachable through, e.g. `"https://relay.example.org"`.
    pub relays: Vec<String>,
}

impl PeerAddr {
    fn parse(&self) -> Result<kuri_link::PeerAddr, LinkError> {
        let id = PeerId::from_str(&self.id).map_err(|e| LinkError::new(format!("peer id: {e}")))?;
        let mut addr = kuri_link::PeerAddr::new(id);
        for a in &self.addrs {
            let socket =
                SocketAddr::from_str(a).map_err(|e| LinkError::new(format!("address {a}: {e}")))?;
            addr = addr.with_addr(socket);
        }
        for r in &self.relays {
            let relay = RelayUrl::from_str(r).map_err(|e| LinkError::new(e.to_string()))?;
            addr = addr.with_relay(relay);
        }
        Ok(addr)
    }

    fn from_core(addr: &kuri_link::PeerAddr) -> PeerAddr {
        PeerAddr {
            id: addr.id.to_string(),
            addrs: addr.addrs.iter().map(|a| a.to_string()).collect(),
            relays: addr.relays.iter().map(|u| u.to_string()).collect(),
        }
    }
}

/// Where an app's back end runs. The front end's code is the same either way;
/// this is a configuration choice, and it can change between launches.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Linked into this app and registered with `kuri_link::register_local` by the
    /// app's own Rust code at start-up. Works offline; no key, no network.
    Local,
    /// On another device, reached over iroh.
    Remote { peer: PeerAddr },
}

/// Why a request failed.
#[derive(Debug, Clone)]
#[frb(dart_code = "
  @override
  String toString() => fault == null ? 'LinkError: $message' : 'LinkError(${fault!.name}): $message';
")]
pub struct LinkError {
    /// Set when the back end answered with a refusal or a failure. Absent when
    /// the request never got an answer: a bad argument, an unreachable peer,
    /// a broken stream.
    pub fault: Option<FaultCode>,
    pub message: String,
}

impl LinkError {
    fn new(message: impl Into<String>) -> LinkError {
        LinkError {
            fault: None,
            message: message.into(),
        }
    }

    fn other(e: impl fmt::Display) -> LinkError {
        LinkError::new(e.to_string())
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.fault {
            Some(code) => write!(f, "{code:?}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for LinkError {}

impl From<CallError> for LinkError {
    fn from(e: CallError) -> LinkError {
        match e {
            CallError::Fault(fault) => LinkError {
                fault: Some(fault.code.into()),
                message: fault.message,
            },
            other => LinkError::other(other),
        }
    }
}

/// What kind of refusal or failure the back end answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultCode {
    /// The request was malformed, or its arguments did not fit the method.
    BadRequest,
    /// The back end does not speak this wire version.
    UnsupportedVersion,
    /// The back end refused this caller: pair first.
    Denied,
    /// The app serves no method by that name (for that operation). On the
    /// wire this is `no_such_method`, a name Dart reserves.
    MethodNotFound,
    /// The method ran and failed.
    App,
    /// A code from a newer back end.
    Unknown,
}

impl From<kuri_link::FaultCode> for FaultCode {
    fn from(code: kuri_link::FaultCode) -> FaultCode {
        match code {
            kuri_link::FaultCode::BadRequest => FaultCode::BadRequest,
            kuri_link::FaultCode::UnsupportedVersion => FaultCode::UnsupportedVersion,
            kuri_link::FaultCode::Denied => FaultCode::Denied,
            kuri_link::FaultCode::NoSuchMethod => FaultCode::MethodNotFound,
            kuri_link::FaultCode::App => FaultCode::App,
            kuri_link::FaultCode::Unknown => FaultCode::Unknown,
        }
    }
}

/// This device's node. One per app process is enough.
pub struct LinkNode {
    inner: Node,
    relays: Vec<String>,
}

impl LinkNode {
    /// Starts a node whose identity lives in the file at `key_path`.
    /// `relays` empty means direct paths only. A node uses only the relays it
    /// started with, so to reach a back end through its relays, start the node
    /// with them: see `ticket_back_end`.
    pub async fn start(key_path: String, relays: Vec<String>) -> Result<LinkNode, LinkError> {
        let key = kuri_link::load_or_create_key(&PathBuf::from(&key_path))
            .map_err(|e| LinkError::new(format!("key file {key_path}: {e}")))?;
        let urls = relays
            .iter()
            .map(|r| RelayUrl::from_str(r).map_err(LinkError::other))
            .collect::<Result<Vec<_>, _>>()?;
        let relays = urls.iter().map(|u| u.to_string()).collect();
        let mode = if urls.is_empty() {
            Relays::None
        } else {
            Relays::Custom(urls)
        };
        let inner = Node::builder()
            .secret_key(key)
            .relays(mode)
            .bind()
            .await
            .map_err(LinkError::other)?;
        Ok(LinkNode { inner, relays })
    }

    /// The relays this node started with, in the form a `PeerAddr` lists them.
    #[frb(sync)]
    pub fn relays(&self) -> Vec<String> {
        self.relays.clone()
    }

    /// Tells the node that the device's network may have changed. On Android,
    /// where the node cannot see that by itself, call it whenever the platform
    /// reports a change of network; without it, a switch between WiFi and
    /// mobile data stalls requests until the old path times out. Calling it
    /// when nothing changed does no harm.
    pub async fn network_changed(&self) {
        self.inner.network_change().await;
    }

    /// Closes the node. Its key stays in its file, so a node started again
    /// from that file is the same device to every back end it paired with.
    pub async fn shutdown(self) {
        self.inner.shutdown().await;
    }

    /// This device's public key: how a back end knows it once paired.
    #[frb(sync)]
    pub fn id(&self) -> String {
        self.inner.id().to_string()
    }

    /// Pairs this device with the back end whose QR code read `ticket`.
    /// Returns the back end's address: keep it, and use it with
    /// `Backend::Remote` from then on. A ticket admits one device.
    pub async fn pair(&self, ticket: String) -> Result<PeerAddr, LinkError> {
        let ticket = Ticket::from_str(&ticket).map_err(LinkError::other)?;
        let addr = self.inner.pair(&ticket).await.map_err(|e| match e {
            kuri_link::PairError::Call(e) => LinkError::from(e),
            other => LinkError::other(other),
        })?;
        Ok(PeerAddr::from_core(&addr))
    }

    /// Opens `app` on whichever back end the configuration names.
    pub async fn open_app(
        &self,
        app: String,
        backend: Backend,
        credential: Option<Vec<u8>>,
    ) -> Result<LinkClient, LinkError> {
        match backend {
            Backend::Local => open_local(app),
            Backend::Remote { peer } => self.connect(peer, app, credential).await,
        }
    }

    pub async fn connect(
        &self,
        peer: PeerAddr,
        app: String,
        credential: Option<Vec<u8>>,
    ) -> Result<LinkClient, LinkError> {
        let addr = peer.parse()?;
        let mut client = self
            .inner
            .connect(addr, &app)
            .await
            .map_err(LinkError::other)?;
        if let Some(credential) = credential {
            client = client.with_credential(credential);
        }
        Ok(LinkClient { inner: client })
    }
}

/// The back end a pairing ticket names, read without redeeming the ticket.
/// An app starts its node with the relays listed here before it pairs, so the
/// node can reach a back end that is only reachable through them.
#[frb(sync)]
pub fn ticket_back_end(ticket: String) -> Result<PeerAddr, LinkError> {
    let ticket = Ticket::from_str(&ticket).map_err(LinkError::other)?;
    Ok(PeerAddr::from_core(&ticket.addr))
}

/// Opens the in-process back end registered as `app`.
pub fn open_local(app: String) -> Result<LinkClient, LinkError> {
    kuri_link::Client::local(&app)
        .map(|inner| LinkClient { inner })
        .ok_or_else(|| LinkError::new(format!("no local back end is registered as {app}")))
}

/// A connection to one application's back end, local or remote.
pub struct LinkClient {
    inner: kuri_link::Client,
}

/// Which way a connection's data goes. A kind of path added in a later 0.1.x
/// release shows up as no path (`LinkClient::path` returns `None`), not as a
/// new variant; new variants wait for the next 0.x release.
#[derive(Debug, Clone)]
pub enum LinkRoute {
    /// Straight to the peer at this socket address, e.g. `"192.0.2.40:41234"`.
    Direct { addr: String },
    /// Through this relay, which forwards packets it cannot read.
    Relay { url: String },
}

/// The path a remote client's requests use at one moment.
#[derive(Debug, Clone)]
pub struct LinkPath {
    pub route: LinkRoute,
    /// QUIC's current estimate of the round-trip time, in microseconds.
    pub rtt_micros: u32,
}

impl LinkClient {
    /// `true` when the back end runs in this process.
    #[frb(sync)]
    pub fn is_local(&self) -> bool {
        self.inner.peer().is_none()
    }

    /// The path requests use now: direct or through a relay. `None` in
    /// process, while the connection has no path selected, or once it is lost;
    /// the next request then dials again. iroh moves a connection from a relay
    /// to a direct path once hole punching succeeds, and back when the network
    /// changes, so ask again to follow it. A kind of path this release has no
    /// `LinkRoute` for is also `None`.
    #[frb(sync)]
    pub fn path(&self) -> Option<LinkPath> {
        let path = self.inner.path()?;
        let route = match path.route {
            kuri_link::Route::Direct(addr) => LinkRoute::Direct {
                addr: addr.to_string(),
            },
            kuri_link::Route::Relay(url) => LinkRoute::Relay {
                url: url.to_string(),
            },
            _ => return None,
        };
        Some(LinkPath {
            route,
            rtt_micros: path.rtt.as_micros().try_into().unwrap_or(u32::MAX),
        })
    }

    /// The back end's address as this client knows it: the one it was opened
    /// with, replaced whenever the back end tells of a change. A back end
    /// tells a client its address after the client's first request, and again
    /// whenever it changes, say when the ISP gives the home a new IP; the
    /// client dials it from then on. Keep it in place of the stored one, so
    /// that the next launch dials it too. `None` in process.
    #[frb(sync)]
    pub fn back_end(&self) -> Option<PeerAddr> {
        self.inner.peer_addr().as_ref().map(PeerAddr::from_core)
    }

    /// The changes of `back_end` from now on, one at a time: see
    /// `BackEndUpdates::next`. `None` in process.
    #[frb(sync)]
    pub fn back_end_updates(&self) -> Option<BackEndUpdates> {
        self.inner
            .peer_addr_updates()
            .map(|rx| BackEndUpdates { rx: Mutex::new(rx) })
    }

    /// Calls `method` with CBOR-encoded `args`; returns the CBOR-encoded result.
    /// Empty `args` stands for CBOR null.
    pub async fn call(&self, method: String, args_cbor: Vec<u8>) -> Result<Vec<u8>, LinkError> {
        let out = self.inner.call(&method, decode_args(&args_cbor)?).await?;
        let mut buf = Vec::new();
        ciborium::into_writer(&out, &mut buf)
            .map_err(|e| LinkError::other(format!("result: {e}")))?;
        Ok(buf)
    }

    /// Opens a byte stream to `method`, a method the back end serves with
    /// `Methods::duplex`. `args_cbor` is as for [`LinkClient::call`].
    pub async fn open(&self, method: String, args_cbor: Vec<u8>) -> Result<LinkStream, LinkError> {
        let io = self.inner.open(&method, decode_args(&args_cbor)?).await?;
        Ok(LinkStream {
            send: Mutex::new(io.send),
            recv: Mutex::new(io.recv),
        })
    }
}

/// Follows a remote client's `back_end`. It does not keep the client open:
/// once the client is disposed, `next` returns `None`.
pub struct BackEndUpdates {
    rx: Mutex<tokio::sync::watch::Receiver<kuri_link::PeerAddr>>,
}

impl BackEndUpdates {
    /// Waits for the back end's address to change and returns the new one;
    /// `None` once the client is gone.
    pub async fn next(&self) -> Option<PeerAddr> {
        let mut rx = self.rx.lock().await;
        rx.changed().await.ok()?;
        let addr = PeerAddr::from_core(&rx.borrow_and_update());
        Some(addr)
    }
}

/// The most one [`LinkStream::read`] returns.
const MAX_READ: u32 = 1 << 20;

fn decode_args(args_cbor: &[u8]) -> Result<Value, LinkError> {
    if args_cbor.is_empty() {
        return Ok(Value::Null);
    }
    ciborium::from_reader(args_cbor).map_err(|e| LinkError::new(format!("args: {e}")))
}

/// Both directions of an opened method. Writing and reading can run at the
/// same time; [`LinkStream::finish`] ends what this side sends.
pub struct LinkStream {
    send: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    recv: Mutex<Box<dyn AsyncRead + Send + Unpin>>,
}

impl LinkStream {
    /// Sends `data`, all of it.
    pub async fn write(&self, data: Vec<u8>) -> Result<(), LinkError> {
        let mut send = self.send.lock().await;
        send.write_all(&data).await.map_err(LinkError::other)
    }

    /// Ends this side of the stream: the back end reads end-of-stream.
    pub async fn finish(&self) -> Result<(), LinkError> {
        let mut send = self.send.lock().await;
        send.shutdown().await.map_err(LinkError::other)
    }

    /// Up to `max` bytes, and never more than 1 MiB, as soon as any arrive.
    /// Empty means the back end finished its side.
    pub async fn read(&self, max: u32) -> Result<Vec<u8>, LinkError> {
        // The buffer is allocated before anything arrives, so a large `max`
        // must not become a large allocation.
        let mut buf = vec![0; max.min(MAX_READ) as usize];
        let n = self
            .recv
            .lock()
            .await
            .read(&mut buf)
            .await
            .map_err(LinkError::other)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Everything until the back end finishes its side.
    pub async fn read_to_end(&self) -> Result<Vec<u8>, LinkError> {
        let mut buf = Vec::new();
        self.recv
            .lock()
            .await
            .read_to_end(&mut buf)
            .await
            .map_err(LinkError::other)?;
        Ok(buf)
    }
}
