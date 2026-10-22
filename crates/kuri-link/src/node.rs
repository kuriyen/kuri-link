//! A node: one iroh endpoint that can serve applications and call others.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use ciborium::Value;
use iroh::endpoint::{presets, BindOpts, Connection, PortmapperConfig};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, RelayMode, TransportAddr, Watcher};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::{oneshot, watch};

use crate::addr::{AddrWire, NodeKey, PeerAddr, PeerId, RelayUrl};
use crate::hooks::{AuthRequest, Authorizer, Caller, DenyAll, Origin, Waker};
use crate::pairing::{Pairing, PAIR_APP};
use crate::portmap::{is_global, is_public, PortMapper};
use crate::service::{from_value, to_value, AppError, Duplex, Service};
use crate::wire::{
    read_frame, write_frame, Fault, FaultCode, Header, Op, Reply, WireError, DEFAULT_MAX_FRAME,
    WIRE_VERSION,
};

/// The method a back end answers itself, on every application it serves: it
/// sends the caller its own address, then each change of it. A remote
/// [`Client`] asks for it once it makes a request, and dials the address it
/// learns from then on ([`Client::peer_addr`]).
///
/// Reserved (see [`RESERVED_PREFIX`]): a [`Service`] never sees a request
/// for it. An [`Authorizer`] is asked about it like any other method, and
/// asked again before each change is sent; one that refuses it only keeps the
/// caller from learning of changes.
pub const ADDR_METHOD: &str = "kuri-link.addr";

/// The prefix of the method names Kuri Link keeps for itself, such as
/// [`ADDR_METHOD`], which every application a node serves answers. A request
/// for one never reaches an application's [`Service`], and
/// [`NodeBuilder::bind`] refuses a service that lists one among its
/// [`Service::method_names`].
pub const RESERVED_PREFIX: &str = "kuri-link.";

/// The DNS-SD service a node announces itself under on the local network.
const MDNS_SERVICE: &str = "kuri-link";

/// The shortest gap between two of a back end's address updates, however
/// often its addresses churn.
const ADDR_UPDATE_GAP: Duration = Duration::from_secs(1);

/// How long to wait for a back end whose every address is on its own
/// network. On that network it answers within milliseconds; from anywhere
/// else it never does, and iroh would wait 30 seconds to say so.
const LOCAL_DIAL: Duration = Duration::from_secs(5);

/// Which relays a node may use when no direct path exists.
///
/// There is no default: choosing a relay operator is choosing who sees your
/// connection metadata (who talks to whom, when, how much — never content,
/// which is end-to-end encrypted). So it is always an explicit choice.
#[derive(Debug, Clone)]
pub enum Relays {
    /// Direct paths only: LAN, or addresses known out of band.
    None,
    /// Relays you run, e.g. `iroh-relay` on your own host.
    Custom(Vec<RelayUrl>),
    /// The public relays operated by the iroh maintainers.
    PublicDefault,
}

/// Errors from binding a node or connecting to a peer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("binding the endpoint: {0}")]
    Bind(String),
    #[error("connecting to {peer} for {app}: {reason}")]
    Connect {
        peer: String,
        app: String,
        reason: String,
    },
}

/// A request that did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The server answered with a refusal or a failure.
    #[error(transparent)]
    Fault(#[from] Fault),
    /// The request never completed at the wire level.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The result did not decode into the requested type.
    #[error("decoding the result: {0}")]
    Decode(String),
}

/// An application a node serves: its name, its service, and who may call it.
type Served = (Arc<str>, Arc<dyn Service>, Arc<dyn Authorizer>);

/// Configures and binds a [`Node`].
pub struct NodeBuilder {
    key: Option<NodeKey>,
    relays: Relays,
    bind: Option<SocketAddr>,
    port: Option<u16>,
    port_mapping: bool,
    /// The NAT-PMP router to ask instead of the default gateway's.
    #[cfg(test)]
    nat_pmp: Option<std::net::SocketAddrV4>,
    local_discovery: bool,
    authorizer: Option<Arc<dyn Authorizer>>,
    pairing: Option<Pairing>,
    waker: Option<Arc<dyn Waker>>,
    apps: Vec<(String, Arc<dyn Service>)>,
    max_frame: usize,
}

impl Default for NodeBuilder {
    fn default() -> Self {
        NodeBuilder {
            key: None,
            relays: Relays::None,
            bind: None,
            port: None,
            port_mapping: false,
            #[cfg(test)]
            nat_pmp: None,
            local_discovery: false,
            authorizer: None,
            pairing: None,
            waker: None,
            apps: Vec::new(),
            max_frame: DEFAULT_MAX_FRAME,
        }
    }
}

impl NodeBuilder {
    /// The node's identity. Without one, a fresh key is generated and the
    /// node is reachable only for this process's lifetime.
    pub fn secret_key(mut self, key: NodeKey) -> Self {
        self.key = Some(key);
        self
    }

    pub fn relays(mut self, relays: Relays) -> Self {
        self.relays = relays;
        self
    }

    /// Binds only a UDP socket at `addr`, instead of iroh's default sockets on
    /// every interface of both address families.
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind = Some(addr);
        self
    }

    /// Binds iroh's sockets, on every interface of both address families, on
    /// `port` instead of a random port. Ignored with [`NodeBuilder::bind_addr`],
    /// whose address carries its own port.
    ///
    /// A back end that keeps its port keeps its address: the router is asked
    /// to forward the same port from outside (see
    /// [`NodeBuilder::port_mapping`]), so the public address in the tickets it
    /// issued stays good after a restart. Choose a port once, say at random
    /// above 1024, and keep it with the node's key.
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Whether to ask the router to forward this node's port from outside, by
    /// NAT-PMP or UPnP, so that devices away from home can dial it directly.
    /// Off by default: off, the node sends the router no NAT-PMP, UPnP or PCP
    /// request at all.
    ///
    /// Turned on, the router holds an inbound port that leads to this
    /// machine, for as long as the node runs and up to two hours after it
    /// stops without [`Node::shutdown`]. A router that grants only permanent
    /// mappings, which would outlive the node, is not used.
    ///
    /// A node that serves an application asks for the outside port to equal
    /// its own, keeps the mapping renewed, and lists the outside address in
    /// [`Node::addr`]; [`Node::reach`] says whether the router granted it. A
    /// node that serves nothing leaves mapping to iroh, which lets the router
    /// pick the port.
    pub fn port_mapping(mut self, on: bool) -> Self {
        self.port_mapping = on;
        self
    }

    /// Whether to use mDNS on the local network. Off by default: off, the
    /// node neither announces itself nor looks peers up, and sends no mDNS
    /// packet.
    ///
    /// Turned on, a node that serves an application announces its key and
    /// addresses there, so devices at home find it even after its address on
    /// the home network changed. Everyone on that network then sees the key
    /// and addresses, though only devices the node admits get any further.
    /// Every node with it on looks peers up that way when it dials, but only a
    /// node that serves announces itself. A node bound to loopback does
    /// neither.
    pub fn local_discovery(mut self, on: bool) -> Self {
        self.local_discovery = on;
        self
    }

    /// Who may call this node's applications. Defaults to [`DenyAll`], or,
    /// with [`NodeBuilder::pairing`], to the peers paired so far.
    pub fn authorizer(mut self, authorizer: impl Authorizer) -> Self {
        self.authorizer = Some(Arc::new(authorizer));
        self
    }

    /// Lets devices pair with this node by redeeming tickets that `pairing`
    /// issues.
    ///
    /// The node also serves [`PAIR_APP`], where a live ticket admits its
    /// holder to one call that adds the holder's key to the pairing's
    /// [`PeerSet`](crate::PeerSet). That door is guarded by the pairing alone.
    /// The node's other applications admit that same set, unless
    /// [`NodeBuilder::authorizer`] names an authorizer of its own.
    pub fn pairing(mut self, pairing: Pairing) -> Self {
        self.pairing = Some(pairing);
        self
    }

    pub fn waker(mut self, waker: impl Waker) -> Self {
        self.waker = Some(Arc::new(waker));
        self
    }

    /// Serves `service` as application `app` (ALPN `kuri-link/<v>/<app>`).
    pub fn serve(mut self, app: &str, service: impl Service) -> Self {
        self.apps.push((app.to_owned(), Arc::new(service)));
        self
    }

    pub fn max_frame(mut self, bytes: usize) -> Self {
        self.max_frame = bytes;
        self
    }

    pub async fn bind(self) -> Result<Node, Error> {
        let authorizer: Arc<dyn Authorizer> = match (self.authorizer, &self.pairing) {
            (Some(authorizer), _) => authorizer,
            (None, Some(pairing)) => Arc::new(pairing.peers().clone()),
            (None, None) => Arc::new(DenyAll),
        };
        let max_frame = self.max_frame;
        for (app, service) in &self.apps {
            if let Some(name) = service
                .method_names()
                .into_iter()
                .find(|name| name.starts_with(RESERVED_PREFIX))
            {
                return Err(Error::Bind(format!(
                    "application {app:?}: method names starting with \
                     {RESERVED_PREFIX:?}, such as {name:?}, are reserved for Kuri Link"
                )));
            }
        }
        let mut apps: Vec<Served> = self
            .apps
            .into_iter()
            .map(|(app, service)| (app.into(), service, authorizer.clone()))
            .collect();
        if let Some(pairing) = self.pairing {
            if apps.iter().any(|(app, _, _)| &**app == PAIR_APP) {
                return Err(Error::Bind(format!(
                    "the application name {PAIR_APP:?} is reserved for pairing"
                )));
            }
            apps.push((
                PAIR_APP.into(),
                Arc::new(pairing.service()),
                Arc::new(pairing),
            ));
        }
        let serves = !apps.is_empty();
        let configured_relays: Arc<[RelayUrl]> = match &self.relays {
            Relays::Custom(urls) => urls.clone().into(),
            Relays::None | Relays::PublicDefault => Arc::new([]),
        };

        // A back end maps its own port (see `portmap`), so iroh must not map
        // it as well.
        let maps_own_port = self.port_mapping && serves;
        let iroh_maps = self.port_mapping && !maps_own_port;
        let mut builder = Endpoint::builder(presets::Minimal);
        if !iroh_maps {
            builder = builder.portmapper_config(PortmapperConfig::Disabled);
        }
        if let Some(key) = self.key {
            builder = builder.secret_key(key.into_iroh());
        }
        builder = match self.relays {
            Relays::None => builder.relay_mode(RelayMode::Disabled),
            Relays::Custom(urls) => {
                builder.relay_mode(RelayMode::custom(urls.iter().map(RelayUrl::to_iroh)))
            }
            Relays::PublicDefault => builder.relay_mode(RelayMode::Default),
        };
        let bind_error = |e: iroh::endpoint::InvalidSocketAddr| Error::Bind(e.to_string());
        match (self.bind, self.port) {
            (Some(addr), _) => {
                // On its own, iroh's bind_addr replaces only the default
                // socket of the same address family; the other family's stays
                // on every interface.
                builder = builder
                    .clear_ip_transports()
                    .bind_addr(addr)
                    .map_err(bind_error)?;
            }
            (None, Some(port)) => {
                // As iroh's defaults: IPv4 must bind, IPv6 may fail.
                builder = builder
                    .clear_ip_transports()
                    .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
                    .map_err(bind_error)?
                    .bind_addr_with_opts(
                        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
                        BindOpts::default().set_is_required(false),
                    )
                    .map_err(bind_error)?;
            }
            (None, None) => {}
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| Error::Bind(e.to_string()))?;

        let on_loopback = self.bind.is_some_and(|addr| addr.ip().is_loopback());
        if self.local_discovery && !on_loopback {
            // Added after binding, so that a network without multicast costs
            // discovery and nothing else.
            let mdns = MdnsAddressLookup::builder()
                .service_name(MDNS_SERVICE)
                .advertise(serves)
                .build(endpoint.id());
            match (mdns, endpoint.address_lookup()) {
                (Ok(mdns), Ok(lookup)) => lookup.add(mdns),
                (Err(e), _) => tracing::info!(error = %e, "no discovery on the local network"),
                (_, Err(e)) => tracing::info!(error = %e, "no discovery on the local network"),
            }
        }

        #[cfg(test)]
        let nat_pmp = self.nat_pmp;
        #[cfg(not(test))]
        let nat_pmp = None;
        // The router can forward only to an IPv4 socket it can reach: not
        // one on loopback.
        let unmapped = match (self.port_mapping, iroh_maps) {
            (false, _) => "port mapping is off",
            (true, true) => "the router forwarded no port",
            (true, false) => "no IPv4 socket that a router can forward to",
        };
        let mapper = if maps_own_port {
            endpoint
                .bound_sockets()
                .into_iter()
                .find_map(|socket| match socket {
                    SocketAddr::V4(v4) if !v4.ip().is_loopback() => Some(v4),
                    _ => None,
                })
                .map(|v4| {
                    let local_ip = Some(*v4.ip()).filter(|ip| !ip.is_unspecified());
                    PortMapper::spawn(endpoint.clone(), local_ip, v4.port(), nat_pmp)
                })
        } else {
            None
        };

        let router = if serves {
            let own = own_addr(&endpoint, configured_relays);
            let mut router = Router::builder(endpoint.clone());
            for (app, service, authorizer) in apps {
                let alpn = crate::alpn(&app);
                let handler = AppHandler {
                    app,
                    service,
                    authorizer,
                    own: own.clone(),
                    max_frame,
                };
                router = router.accept(alpn, handler);
            }
            Some(router.spawn())
        } else {
            None
        };

        Ok(Node {
            endpoint,
            router,
            mapper,
            unmapped,
            iroh_maps,
            waker: self.waker,
            max_frame,
        })
    }
}

/// One iroh endpoint, serving zero or more applications.
#[derive(Debug)]
pub struct Node {
    endpoint: Endpoint,
    router: Option<Router>,
    /// Keeps a back end's port forwarded by the router.
    mapper: Option<PortMapper>,
    /// Why no port is forwarded, when there is no mapper.
    unmapped: &'static str,
    /// Whether iroh asks the router for a mapping instead.
    iroh_maps: bool,
    waker: Option<Arc<dyn Waker>>,
    max_frame: usize,
}

impl std::fmt::Debug for dyn Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

impl Node {
    pub fn builder() -> NodeBuilder {
        NodeBuilder::default()
    }

    /// The public key peers dial.
    pub fn id(&self) -> PeerId {
        PeerId::from_iroh(self.endpoint.id())
    }

    /// Everything iroh currently knows about how to reach this node.
    pub fn addr(&self) -> PeerAddr {
        PeerAddr::from_iroh(&self.endpoint.addr())
    }

    /// This node's key plus the sockets it is bound to: enough for a peer on
    /// the same host or LAN to dial it with no relay and no lookup service.
    /// A socket bound to every interface (`0.0.0.0` or `[::]`) names no
    /// address a peer can dial and is left out; for such a node use
    /// [`Node::addr`], which lists the interfaces' addresses.
    pub fn bound_addr(&self) -> PeerAddr {
        let sockets = self.endpoint.bound_sockets();
        let mut addr = PeerAddr::new(self.id());
        for socket in sockets.into_iter().filter(|s| !s.ip().is_unspecified()) {
            addr = addr.with_addr(socket);
        }
        addr
    }

    /// Whether devices outside this node's network can reach it. Waits up to
    /// `timeout` for the router to answer a request to forward the node's
    /// port, which on a home network takes well under a second; a back end
    /// asks before it issues a ticket, so that the ticket carries the public
    /// address.
    ///
    /// Only IPv4 is judged. A global IPv6 address is listed in
    /// [`Node::addr`] all the same, but whether the router's firewall lets
    /// devices outside reach it cannot be seen from inside.
    pub async fn reach(&self, timeout: Duration) -> Reach {
        let deadline = tokio::time::Instant::now() + timeout;
        let mapped = match &self.mapper {
            Some(mapper) => mapper.settled(deadline).await.map(SocketAddr::V4),
            None => Err(self.unmapped.to_string()),
        };
        if let Some(external) = mapped.as_ref().ok().filter(|a| is_public(a)) {
            return Reach::Public(*external);
        }
        let own_public = |addr: &iroh::EndpointAddr| addr.ip_addrs().copied().find(is_public);
        let mut watch = self.endpoint.watch_addr();
        if self.iroh_maps {
            // iroh's mapping, if the router grants one, shows up among the
            // node's addresses.
            while own_public(&watch.get()).is_none() {
                if tokio::time::timeout_at(deadline, watch.updated())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
        // A public address of the node's own: a host with no NAT in front.
        if let Some(public) = own_public(&watch.get()) {
            return Reach::Public(public);
        }
        match mapped {
            Ok(external) => Reach::BehindNat(external),
            Err(reason) => Reach::Unmapped(reason),
        }
    }

    /// Connects to `app` on the peer at `addr`. On failure, a configured
    /// [`Waker`] gets one chance to wake the peer before a single retry.
    ///
    /// When none of `addr`'s addresses can be reached from outside the peer's
    /// own network and it names no relay, connecting gives up after five
    /// seconds rather than iroh's thirty, and says so: the peer can only be
    /// reached from its own network.
    ///
    /// If the connection is lost later, say while a phone is out of signal
    /// for longer than QUIC's idle timeout, the client dials again the same
    /// way before its next request.
    pub async fn connect(&self, addr: impl Into<PeerAddr>, app: &str) -> Result<Client, Error> {
        let addr = addr.into();
        let conn = dial(&self.endpoint, &addr, app, self.waker.as_deref()).await?;
        Ok(Client {
            inner: Inner::Remote(Arc::new(Remote {
                endpoint: self.endpoint.clone(),
                id: addr.id,
                addr: watch::Sender::new(addr),
                app: app.to_owned(),
                waker: self.waker.clone(),
                max_frame: self.max_frame,
                conn: Mutex::new(conn),
                dialing: tokio::sync::Mutex::new(()),
                following: Mutex::new(None),
            })),
            credential: None,
        })
    }

    /// Tells the node that the device's network may have changed, so that it
    /// looks for paths over the new network now instead of waiting for the old
    /// ones to time out. On Android, where native code cannot watch the
    /// network, call it from the platform's network callback; elsewhere iroh
    /// notices by itself. Calling it when nothing changed does no harm.
    pub async fn network_change(&self) {
        self.endpoint.network_change().await;
    }

    /// Stops serving, asks the router to stop forwarding the node's port, and
    /// closes the endpoint.
    pub async fn shutdown(self) {
        if let Some(mapper) = self.mapper {
            mapper.release().await;
        }
        match self.router {
            Some(router) => {
                let _ = router.shutdown().await;
            }
            None => self.endpoint.close().await,
        }
    }
}

/// Connects to `app` at `addr`. On failure, `waker` gets one chance to wake
/// the peer before a single retry.
async fn dial(
    endpoint: &Endpoint,
    addr: &PeerAddr,
    app: &str,
    waker: Option<&dyn Waker>,
) -> Result<Connection, Error> {
    let target = addr.to_iroh();
    let alpn = crate::alpn(app);
    let local = only_local(addr);
    let connect = || async {
        let attempt = endpoint.connect(target.clone(), &alpn);
        if local {
            match tokio::time::timeout(LOCAL_DIAL, attempt).await {
                Ok(done) => done.map_err(|e| e.to_string()),
                Err(_) => Err("timed out".to_owned()),
            }
        } else {
            attempt.await.map_err(|e| e.to_string())
        }
    };
    let first = connect().await;
    match (first, waker) {
        (Ok(conn), _) => Ok(conn),
        (Err(first), Some(waker)) => {
            if waker.wake(addr, app).await {
                connect().await.map_err(|e| connect_error(addr, app, e))
            } else {
                Err(connect_error(addr, app, first))
            }
        }
        (Err(first), None) => Err(connect_error(addr, app, first)),
    }
}

/// Whether every way to reach `addr` stays inside the peer's own network: no
/// relay, and no address that devices elsewhere could dial.
fn only_local(addr: &PeerAddr) -> bool {
    addr.relays.is_empty() && !addr.addrs.iter().any(is_global)
}

fn connect_error(addr: &PeerAddr, app: &str, reason: String) -> Error {
    let reason = if only_local(addr) {
        format!(
            "{reason}; it has no address outside its own network and no relay, \
             so only a device on that network can reach it"
        )
    } else {
        reason
    };
    Error::Connect {
        peer: addr.id.fmt_short(),
        app: app.to_owned(),
        reason,
    }
}

/// A handle on one application's back end — on a peer, or in this process.
///
/// The front end does not care which: [`Client::call`] and [`Client::open`]
/// behave the same either way, with the same faults. That is what lets one app
/// switch between a back end on the device and one across the network by
/// configuration alone. Cheap to clone; requests run concurrently.
#[derive(Clone)]
pub struct Client {
    inner: Inner,
    credential: Option<Vec<u8>>,
}

#[derive(Clone)]
enum Inner {
    Remote(Arc<Remote>),
    Local { service: Arc<dyn Service> },
}

/// A back end on a peer: the connection to it, and what it takes to dial
/// again once that connection is lost.
struct Remote {
    endpoint: Endpoint,
    id: PeerId,
    /// The peer's address as last known: what it was dialed with, until the
    /// peer tells of a change. Dialed again from here.
    addr: watch::Sender<PeerAddr>,
    app: String,
    waker: Option<Arc<dyn Waker>>,
    max_frame: usize,
    conn: Mutex<Connection>,
    /// Held while dialing, so that requests which find the connection lost
    /// at the same time dial once between them.
    dialing: tokio::sync::Mutex<()>,
    /// The connection whose address updates are being read, and what stops
    /// that reading: dropped with a newer connection, or with this.
    following: Mutex<Option<(usize, oneshot::Sender<()>)>>,
}

impl Remote {
    fn current(&self) -> Connection {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The connection for a new request, dialing again first if the last one
    /// was lost. A request already under way when its connection is lost
    /// fails instead of being sent again, since it may have taken effect.
    async fn connection(&self) -> Result<Connection, WireError> {
        let conn = self.current();
        if conn.close_reason().is_none() {
            return Ok(conn);
        }
        let _dialing = self.dialing.lock().await;
        let conn = self.current();
        if conn.close_reason().is_none() {
            // Another request dialed while this one waited.
            return Ok(conn);
        }
        let addr = self.addr.borrow().clone();
        let conn = dial(&self.endpoint, &addr, &self.app, self.waker.as_deref())
            .await
            .map_err(|e| WireError::Stream(e.to_string()))?;
        *self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = conn.clone();
        Ok(conn)
    }

    /// Starts reading the peer's address updates on `conn`, unless they are
    /// read there already. `header` asks for them, with the credential of the
    /// client whose request comes first.
    fn follow(self: &Arc<Self>, conn: &Connection, header: Header) {
        let mut following = lock(&self.following);
        if following
            .as_ref()
            .is_some_and(|(id, _)| *id == conn.stable_id())
        {
            return;
        }
        let (stop, stopped) = oneshot::channel();
        *following = Some((conn.stable_id(), stop));
        tokio::spawn(read_addr_updates(
            Arc::downgrade(self),
            conn.clone(),
            header,
            self.max_frame,
            stopped,
        ));
    }

    /// Takes `addr` as the peer's address, if it is one.
    fn learn(&self, addr: PeerAddr) {
        // An empty address, say in the middle of a network change, says
        // nothing about where the peer will be.
        if addr.id != self.id || (addr.addrs.is_empty() && addr.relays.is_empty()) {
            return;
        }
        self.addr.send_if_modified(|known| {
            if *known == addr {
                return false;
            }
            tracing::debug!(peer = %self.id.fmt_short(), ?addr, "the peer's address changed");
            *known = addr;
            true
        });
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Asks the peer on `conn` for [`ADDR_METHOD`] and keeps what it sends in
/// `remote`, until the peer stops sending, the connection ends, or `stopped`
/// fires. A peer from before address updates refuses the method, which ends
/// it at once.
async fn read_addr_updates(
    remote: Weak<Remote>,
    conn: Connection,
    header: Header,
    max_frame: usize,
    stopped: oneshot::Receiver<()>,
) {
    let read = async {
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| WireError::Stream(e.to_string()))?;
        write_frame(&mut send, &header, max_frame).await?;
        send.finish()
            .map_err(|e| WireError::Stream(e.to_string()))?;
        if let Reply::Err(fault) = read_frame::<Reply>(&mut recv, max_frame).await? {
            return Err(WireError::Stream(format!("refused: {fault}")));
        }
        loop {
            let update: AddrWire = read_frame(&mut recv, max_frame).await?;
            let Some(remote) = remote.upgrade() else {
                return Ok(());
            };
            match update.to_addr() {
                Ok(addr) => remote.learn(addr),
                Err(e) => tracing::debug!(error = %e, "an address update that does not read"),
            }
        }
    };
    tokio::select! {
        ended = read => {
            if let Err(e) = ended {
                tracing::debug!(error = %e, "address updates ended");
            }
        }
        _ = stopped => {}
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Inner::Remote(remote) => f
                .debug_struct("Client")
                .field("peer", &remote.id.fmt_short().to_string())
                .finish(),
            Inner::Local { .. } => f
                .debug_struct("Client")
                .field("peer", &"in-process")
                .finish(),
        }
    }
}

/// Which way a connection's data goes. More kinds of path may come, so a
/// `match` on it needs a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Route {
    /// Straight to the peer at this address: on the same network, or through
    /// NAT by hole punching.
    Direct(SocketAddr),
    /// Through this relay, which forwards packets it cannot read.
    Relay(RelayUrl),
}

/// Whether devices outside a node's network can reach it, as far as the node
/// can tell from inside. More cases may come, so a `match` on it needs a
/// wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reach {
    /// Devices anywhere can dial this public address: the router forwards it
    /// to the node, or it is the node's own.
    Public(SocketAddr),
    /// The router forwards a port, but from this address, which is not public
    /// itself: the home is behind its ISP's NAT (carrier-grade NAT) or behind
    /// a second router. Devices outside cannot reach the node without a relay.
    BehindNat(SocketAddr),
    /// No port is forwarded, for the reason given: port mapping is off (the
    /// default), or the router does not speak NAT-PMP or UPnP, has them
    /// turned off, or grants only permanent mappings. Devices outside cannot
    /// reach the node without a relay.
    Unmapped(String),
}

/// The path a connection uses at one moment. More fields may come.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PathInfo {
    pub route: Route,
    /// QUIC's current estimate of the round-trip time on this path.
    pub rtt: Duration,
}

/// Size of the in-memory pipe behind an in-process `open`.
const LOCAL_PIPE: usize = 64 * 1024;

impl Client {
    /// A client for a back end linked into this process. No network, no key,
    /// no authorizer: the process is the trust boundary.
    pub fn in_process(service: impl Service) -> Client {
        Client::in_process_arc(Arc::new(service))
    }

    pub fn in_process_arc(service: Arc<dyn Service>) -> Client {
        Client {
            inner: Inner::Local { service },
            credential: None,
        }
    }

    /// A client for the back end registered under `app` with
    /// [`crate::register_local`], if there is one.
    pub fn local(app: &str) -> Option<Client> {
        crate::local::lookup(app).map(Client::in_process_arc)
    }

    /// Attaches a credential to every request from this client. Kuri Link does
    /// not read it; the server's [`Authorizer`] does. Ignored in-process.
    pub fn with_credential(mut self, credential: Vec<u8>) -> Self {
        self.credential = Some(credential);
        self
    }

    /// The peer's key, or `None` when the back end is in this process.
    pub fn peer(&self) -> Option<PeerId> {
        match &self.inner {
            Inner::Remote(remote) => Some(remote.id),
            Inner::Local { .. } => None,
        }
    }

    /// The back end's address as this client knows it: the one it was dialed
    /// with, replaced whenever the back end tells of a change. A back end
    /// tells a client its address after the client's first request, and again
    /// whenever it changes, say when the ISP gives the home a new IP. The
    /// client dials it from then on. Keep it, and connect to it next time.
    /// `None` in process.
    pub fn peer_addr(&self) -> Option<PeerAddr> {
        match &self.inner {
            Inner::Remote(remote) => Some(remote.addr.borrow().clone()),
            Inner::Local { .. } => None,
        }
    }

    /// Sees each new value of [`Client::peer_addr`], to keep it as it
    /// changes. The receiver's `changed()` fails once every clone of this
    /// client is gone. `None` in process.
    pub fn peer_addr_updates(&self) -> Option<watch::Receiver<PeerAddr>> {
        match &self.inner {
            Inner::Remote(remote) => Some(remote.addr.subscribe()),
            Inner::Local { .. } => None,
        }
    }

    /// The connection for a request to a remote back end, which also starts
    /// reading the back end's address updates there.
    async fn connection(&self, remote: &Arc<Remote>) -> Result<Connection, WireError> {
        let conn = remote.connection().await?;
        remote.follow(&conn, self.header(Op::Open, ADDR_METHOD, Value::Null));
        Ok(conn)
    }

    /// The path requests to a remote back end use now: direct or through a
    /// relay, with its round-trip time. `None` in process, while the
    /// connection has no path selected, or once it is lost; the next request
    /// then dials again.
    ///
    /// A snapshot: iroh moves a connection from a relay to a direct path once
    /// hole punching succeeds, and back when the network changes.
    pub fn path(&self) -> Option<PathInfo> {
        let Inner::Remote(remote) = &self.inner else {
            return None;
        };
        let conn = remote.current();
        if conn.close_reason().is_some() {
            return None;
        }
        let paths = conn.paths();
        let path = paths.iter().find(|p| p.is_selected())?;
        let route = match path.remote_addr() {
            TransportAddr::Ip(addr) => Route::Direct(*addr),
            TransportAddr::Relay(url) => Route::Relay(RelayUrl::from_iroh(url.clone())),
            _ => return None,
        };
        Some(PathInfo {
            route,
            rtt: path.rtt(),
        })
    }

    fn header(&self, op: Op, method: &str, args: Value) -> Header {
        Header {
            v: WIRE_VERSION,
            op,
            method: method.to_owned(),
            args,
            cred: self.credential.clone().map(serde_bytes::ByteBuf::from),
        }
    }

    /// Sends `args` to `method` and waits for its single reply.
    pub async fn call(&self, method: &str, args: Value) -> Result<Value, CallError> {
        match &self.inner {
            Inner::Local { service } => service
                .clone()
                .call(in_process_caller(), method.to_owned(), args)
                .await
                .map_err(|e| CallError::Fault(fault_of(e))),
            Inner::Remote(remote) => {
                let max_frame = remote.max_frame;
                let (mut send, mut recv) = self
                    .connection(remote)
                    .await?
                    .open_bi()
                    .await
                    .map_err(|e| WireError::Stream(e.to_string()))?;
                write_frame(&mut send, &self.header(Op::Call, method, args), max_frame).await?;
                send.finish()
                    .map_err(|e| WireError::Stream(e.to_string()))?;
                match read_frame::<Reply>(&mut recv, max_frame).await? {
                    Reply::Ok(value) => Ok(value),
                    Reply::Err(fault) => Err(fault.into()),
                }
            }
        }
    }

    /// [`Client::call`] with typed arguments and result.
    pub async fn call_typed<A, R>(&self, method: &str, args: &A) -> Result<R, CallError>
    where
        A: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let args = to_value(args).map_err(|e| CallError::Decode(e.to_string()))?;
        let out = self.call(method, args).await?;
        from_value(out).map_err(|e| CallError::Decode(e.to_string()))
    }

    /// Opens a raw byte stream to `method`, once the back end accepts it.
    pub async fn open(&self, method: &str, args: Value) -> Result<Duplex, CallError> {
        match &self.inner {
            Inner::Local { service } => {
                let run = service
                    .clone()
                    .accept_open(in_process_caller(), method.to_owned(), args)
                    .await
                    .map_err(|e| CallError::Fault(fault_of(e)))?;
                let (ours, theirs) = Duplex::pipe(LOCAL_PIPE);
                tokio::spawn(run(theirs));
                Ok(ours)
            }
            Inner::Remote(remote) => {
                let max_frame = remote.max_frame;
                let (mut send, mut recv) = self
                    .connection(remote)
                    .await?
                    .open_bi()
                    .await
                    .map_err(|e| WireError::Stream(e.to_string()))?;
                write_frame(&mut send, &self.header(Op::Open, method, args), max_frame).await?;
                match read_frame::<Reply>(&mut recv, max_frame).await? {
                    Reply::Ok(_) => Ok(Duplex {
                        send: Box::new(send),
                        recv: Box::new(recv),
                    }),
                    Reply::Err(fault) => Err(fault.into()),
                }
            }
        }
    }
}

fn in_process_caller() -> Caller {
    Caller {
        origin: Origin::InProcess,
        claims: Value::Null,
    }
}

/// Serves one application: accepts streams, authorizes each request, and
/// dispatches it to the service.
#[derive(Clone)]
struct AppHandler {
    app: Arc<str>,
    service: Arc<dyn Service>,
    authorizer: Arc<dyn Authorizer>,
    /// This node's address, for [`ADDR_METHOD`].
    own: watch::Receiver<PeerAddr>,
    max_frame: usize,
}

/// Where a node can be reached, kept current until the node closes: what
/// iroh knows, plus the relays the node was told to use, which iroh lists only
/// once it has connected to them. Held by the node's address updates instead
/// of the endpoint itself, which would keep the node's sockets open.
fn own_addr(endpoint: &Endpoint, relays: Arc<[RelayUrl]>) -> watch::Receiver<PeerAddr> {
    let own = move |addr: &iroh::EndpointAddr| {
        let mut own = PeerAddr::from_iroh(addr);
        own.relays.extend(relays.iter().cloned());
        own
    };
    let mut iroh_addr = endpoint.watch_addr();
    let (tx, rx) = watch::channel(own(&iroh_addr.get()));
    let closed = endpoint.closed();
    tokio::spawn(async move {
        let follow = async {
            while let Ok(addr) = iroh_addr.updated().await {
                tx.send_if_modified(|known| {
                    let now = own(&addr);
                    let changed = *known != now;
                    *known = now;
                    changed
                });
            }
        };
        tokio::select! {
            _ = follow => {}
            _ = closed => {}
        }
    });
    rx
}

impl std::fmt::Debug for AppHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppHandler")
            .field("app", &self.app)
            .finish()
    }
}

impl ProtocolHandler for AppHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = PeerId::from_iroh(conn.remote_id());
        while let Ok((send, recv)) = conn.accept_bi().await {
            let this = self.clone();
            tokio::spawn(async move {
                if let Err(e) = this.serve_stream(peer, send, recv).await {
                    tracing::debug!(app = %this.app, peer = %peer.fmt_short(), error = %e, "request failed");
                }
            });
        }
        Ok(())
    }
}

impl AppHandler {
    async fn serve_stream(
        &self,
        peer: PeerId,
        mut send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
    ) -> Result<(), WireError> {
        let max = self.max_frame;
        let header: Header = match read_frame(&mut recv, max).await {
            Ok(header) => header,
            Err(e) => {
                return refuse(
                    &mut send,
                    Fault::new(FaultCode::BadRequest, e.to_string()),
                    max,
                )
                .await;
            }
        };
        if header.v != WIRE_VERSION {
            let fault = Fault::new(
                FaultCode::UnsupportedVersion,
                format!(
                    "wire version {} (this server speaks {WIRE_VERSION})",
                    header.v
                ),
            );
            return refuse(&mut send, fault, max).await;
        }

        let request = AuthRequest {
            peer,
            app: &self.app,
            method: &header.method,
            credential: header.cred.as_deref().map(|c| c.as_slice()),
        };
        let caller = match self.authorizer.authorize(request).await {
            Ok(caller) => caller,
            Err(denied) => {
                return refuse(&mut send, Fault::new(FaultCode::Denied, denied.0), max).await
            }
        };
        if header.op == Op::Open && header.method == ADDR_METHOD {
            return self.send_addr(request, send).await;
        }
        if header.method.starts_with(RESERVED_PREFIX) {
            let fault = fault_of(AppError::NoSuchMethod(header.method));
            return refuse(&mut send, fault, max).await;
        }

        match header.op {
            Op::Call => {
                let reply = match self
                    .service
                    .clone()
                    .call(caller, header.method, header.args)
                    .await
                {
                    Ok(value) => Reply::Ok(value),
                    Err(e) => Reply::Err(fault_of(e)),
                };
                write_frame(&mut send, &reply, max).await?;
                send.finish().map_err(|e| WireError::Stream(e.to_string()))
            }
            Op::Open => {
                match self
                    .service
                    .clone()
                    .accept_open(caller, header.method, header.args)
                    .await
                {
                    Ok(run) => {
                        write_frame(&mut send, &Reply::Ok(Value::Null), max).await?;
                        run(Duplex {
                            send: Box::new(send),
                            recv: Box::new(recv),
                        })
                        .await;
                        Ok(())
                    }
                    Err(e) => refuse(&mut send, fault_of(e), max).await,
                }
            }
        }
    }
}

impl AppHandler {
    /// Serves [`ADDR_METHOD`]: this node's address now, then each change of
    /// it, while the caller listens and its authorizer still admits it.
    async fn send_addr(
        &self,
        request: AuthRequest<'_>,
        mut send: iroh::endpoint::SendStream,
    ) -> Result<(), WireError> {
        let max = self.max_frame;
        write_frame(&mut send, &Reply::Ok(Value::Null), max).await?;
        let mut own = self.own.clone();
        loop {
            let now = own.borrow_and_update().clone();
            write_frame(&mut send, &AddrWire::new(&now), max).await?;
            // Leave as soon as the caller stops listening or the node closes;
            // otherwise wait for a change, then a moment more, so that a
            // burst of changes goes out as one.
            tokio::select! {
                changed = own.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                _ = send.stopped() => return Ok(()),
            }
            tokio::select! {
                _ = tokio::time::sleep(ADDR_UPDATE_GAP) => {}
                _ = send.stopped() => return Ok(()),
            }
            // A device unpaired meanwhile hears of no more changes.
            if self.authorizer.authorize(request).await.is_err() {
                break;
            }
        }
        send.finish().map_err(|e| WireError::Stream(e.to_string()))
    }
}

async fn refuse(
    send: &mut iroh::endpoint::SendStream,
    fault: Fault,
    max: usize,
) -> Result<(), WireError> {
    write_frame(send, &Reply::Err(fault), max).await?;
    send.finish().map_err(|e| WireError::Stream(e.to_string()))
}

fn fault_of(e: AppError) -> Fault {
    match e {
        AppError::NoSuchMethod(m) => Fault::new(FaultCode::NoSuchMethod, m),
        AppError::BadArgs(m) => Fault::new(FaultCode::BadRequest, m),
        AppError::Failed(m) => Fault::new(FaultCode::App, m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_bind_address_is_the_only_socket() {
        let node = Node::builder()
            .relays(Relays::None)
            .bind_addr("127.0.0.1:0".parse().unwrap())
            .bind()
            .await
            .unwrap();
        let sockets = node.endpoint.bound_sockets();
        assert_eq!(sockets.len(), 1, "bound {sockets:?}");
        assert_eq!(sockets[0].ip(), std::net::Ipv4Addr::LOCALHOST);
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_port_is_bound_on_every_interface_of_both_families() {
        // A free port, found by binding and letting go.
        let port = std::net::UdpSocket::bind("0.0.0.0:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let node = Node::builder()
            .port(port)
            .port_mapping(false)
            .bind()
            .await
            .unwrap();
        let sockets = node.endpoint.bound_sockets();
        assert!(
            sockets
                .iter()
                .all(|s| s.port() == port && s.ip().is_unspecified()),
            "bound {sockets:?}"
        );
        assert!(sockets.iter().any(SocketAddr::is_ipv4), "bound {sockets:?}");
        node.shutdown().await;
    }

    /// Binds a back end that asks the NAT-PMP router `nat_pmp`, if it asks
    /// any router at all.
    async fn back_end(builder: NodeBuilder, nat_pmp: &tokio::net::UdpSocket) -> Node {
        let SocketAddr::V4(router) = nat_pmp.local_addr().unwrap() else {
            unreachable!()
        };
        NodeBuilder {
            nat_pmp: Some(router),
            ..builder
        }
        .serve("echo", crate::Methods::new())
        .bind()
        .await
        .unwrap()
    }

    /// Whether `router` hears anything within a second.
    async fn heard(router: &tokio::net::UdpSocket) -> bool {
        let mut buf = [0u8; 64];
        tokio::time::timeout(Duration::from_secs(1), router.recv_from(&mut buf))
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_node_asks_no_router_unless_told_to() {
        let router = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let node = back_end(Node::builder(), &router).await;
        assert!(!heard(&router).await, "a default back end asked the router");
        assert!(node.mapper.is_none() && !node.iroh_maps);
        assert_eq!(
            node.reach(Duration::from_secs(1)).await,
            Reach::Unmapped("port mapping is off".into())
        );
        node.shutdown().await;

        // Nor does a node that serves nothing, which would leave it to iroh.
        let node = Node::builder().bind().await.unwrap();
        assert!(node.mapper.is_none() && !node.iroh_maps);
        node.shutdown().await;

        // Told to, the same back end does ask.
        let node = back_end(Node::builder().port_mapping(true), &router).await;
        assert!(heard(&router).await, "the back end did not ask the router");
        assert!(!node.iroh_maps);
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_node_uses_mdns_only_when_told_to() {
        // On every interface, where mDNS would run.
        let mdns = |node: &Node| node.endpoint.address_lookup().unwrap().len();
        let node = Node::builder()
            .serve("echo", crate::Methods::new())
            .bind()
            .await
            .unwrap();
        assert_eq!(mdns(&node), 0, "a default back end uses mDNS");
        node.shutdown().await;

        let node = Node::builder().bind().await.unwrap();
        assert_eq!(mdns(&node), 0, "a default front end uses mDNS");
        node.shutdown().await;

        let node = Node::builder()
            .local_discovery(true)
            .serve("echo", crate::Methods::new())
            .bind()
            .await
            .unwrap();
        assert_eq!(mdns(&node), 1, "told to, the back end does not use mDNS");
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_back_end_on_loopback_says_at_once_that_it_is_unmapped() {
        let node = Node::builder()
            .bind_addr("127.0.0.1:0".parse().unwrap())
            .port_mapping(true)
            .serve("echo", crate::Methods::new())
            .bind()
            .await
            .unwrap();
        assert!(node.mapper.is_none(), "a router cannot forward to loopback");
        let start = std::time::Instant::now();
        assert_eq!(
            node.reach(Duration::from_secs(5)).await,
            Reach::Unmapped("no IPv4 socket that a router can forward to".into())
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        node.shutdown().await;
    }
}
