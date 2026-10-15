//! A node: one iroh endpoint that can serve applications and call others.

use std::net::SocketAddr;
use std::sync::Arc;

use ciborium::Value;
use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::hooks::{AuthRequest, Authorizer, Caller, DenyAll, Origin, Waker};
use crate::pairing::{Pairing, PAIR_APP};
use crate::service::{from_value, to_value, AppError, Duplex, Service};
use crate::wire::{
    read_frame, write_frame, Fault, FaultCode, Header, Op, Reply, WireError, DEFAULT_MAX_FRAME,
    WIRE_VERSION,
};

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

/// Configures and binds a [`Node`].
pub struct NodeBuilder {
    key: Option<SecretKey>,
    relays: Relays,
    bind: Option<SocketAddr>,
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
    pub fn secret_key(mut self, key: SecretKey) -> Self {
        self.key = Some(key);
        self
    }

    pub fn relays(mut self, relays: Relays) -> Self {
        self.relays = relays;
        self
    }

    /// Binds the UDP socket to `addr` instead of iroh's default sockets.
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind = Some(addr);
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
        let mut handlers: Vec<AppHandler> = self
            .apps
            .into_iter()
            .map(|(app, service)| AppHandler {
                app: app.into(),
                service,
                authorizer: authorizer.clone(),
                max_frame,
            })
            .collect();
        if let Some(pairing) = self.pairing {
            if handlers.iter().any(|h| &*h.app == PAIR_APP) {
                return Err(Error::Bind(format!(
                    "the application name {PAIR_APP:?} is reserved for pairing"
                )));
            }
            handlers.push(AppHandler {
                app: PAIR_APP.into(),
                service: Arc::new(pairing.service()),
                authorizer: Arc::new(pairing),
                max_frame,
            });
        }

        let mut builder = Endpoint::builder(presets::Minimal);
        if let Some(key) = self.key {
            builder = builder.secret_key(key);
        }
        builder = match self.relays {
            Relays::None => builder.relay_mode(RelayMode::Disabled),
            Relays::Custom(urls) => builder.relay_mode(RelayMode::custom(urls)),
            Relays::PublicDefault => builder.relay_mode(RelayMode::Default),
        };
        if let Some(addr) = self.bind {
            builder = builder
                .bind_addr(addr)
                .map_err(|e| Error::Bind(e.to_string()))?;
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| Error::Bind(e.to_string()))?;

        let router = if handlers.is_empty() {
            None
        } else {
            let mut router = Router::builder(endpoint.clone());
            for handler in handlers {
                let alpn = crate::alpn(&handler.app);
                router = router.accept(alpn, handler);
            }
            Some(router.spawn())
        };

        Ok(Node {
            endpoint,
            router,
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
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Everything iroh currently knows about how to reach this node.
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// This node's key plus the sockets it is bound to — enough for a peer on
    /// the same host or LAN to dial it with no relay and no lookup service.
    pub fn bound_addr(&self) -> EndpointAddr {
        let sockets = self.endpoint.bound_sockets();
        let mut addr = EndpointAddr::new(self.id());
        for socket in sockets.into_iter().filter(|s| !s.ip().is_unspecified()) {
            addr = addr.with_ip_addr(socket);
        }
        addr
    }

    /// Connects to `app` on the peer at `addr`. On failure, a configured
    /// [`Waker`] gets one chance to wake the peer before a single retry.
    pub async fn connect(&self, addr: impl Into<EndpointAddr>, app: &str) -> Result<Client, Error> {
        let addr = addr.into();
        let alpn = crate::alpn(app);
        let first = self.endpoint.connect(addr.clone(), &alpn).await;
        let conn = match (first, &self.waker) {
            (Ok(conn), _) => conn,
            (Err(first), Some(waker)) => {
                if waker.wake(&addr, app).await {
                    self.endpoint
                        .connect(addr.clone(), &alpn)
                        .await
                        .map_err(|e| connect_error(&addr, app, e))?
                } else {
                    return Err(connect_error(&addr, app, first));
                }
            }
            (Err(first), None) => return Err(connect_error(&addr, app, first)),
        };
        Ok(Client {
            inner: Inner::Remote {
                conn,
                max_frame: self.max_frame,
            },
            credential: None,
        })
    }

    /// Stops serving and closes the endpoint.
    pub async fn shutdown(self) {
        match self.router {
            Some(router) => {
                let _ = router.shutdown().await;
            }
            None => self.endpoint.close().await,
        }
    }
}

fn connect_error(addr: &EndpointAddr, app: &str, e: impl std::fmt::Display) -> Error {
    Error::Connect {
        peer: addr.id.fmt_short().to_string(),
        app: app.to_owned(),
        reason: e.to_string(),
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
    Remote { conn: Connection, max_frame: usize },
    Local { service: Arc<dyn Service> },
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Inner::Remote { conn, .. } => f
                .debug_struct("Client")
                .field("peer", &conn.remote_id().fmt_short().to_string())
                .finish(),
            Inner::Local { .. } => f
                .debug_struct("Client")
                .field("peer", &"in-process")
                .finish(),
        }
    }
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
    pub fn peer(&self) -> Option<EndpointId> {
        match &self.inner {
            Inner::Remote { conn, .. } => Some(conn.remote_id()),
            Inner::Local { .. } => None,
        }
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
            Inner::Remote { conn, max_frame } => {
                let (mut send, mut recv) = conn
                    .open_bi()
                    .await
                    .map_err(|e| WireError::Stream(e.to_string()))?;
                write_frame(&mut send, &self.header(Op::Call, method, args), *max_frame).await?;
                send.finish()
                    .map_err(|e| WireError::Stream(e.to_string()))?;
                match read_frame::<Reply>(&mut recv, *max_frame).await? {
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
            Inner::Remote { conn, max_frame } => {
                let (mut send, mut recv) = conn
                    .open_bi()
                    .await
                    .map_err(|e| WireError::Stream(e.to_string()))?;
                write_frame(&mut send, &self.header(Op::Open, method, args), *max_frame).await?;
                match read_frame::<Reply>(&mut recv, *max_frame).await? {
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
    max_frame: usize,
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
        let peer = conn.remote_id();
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
        peer: EndpointId,
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
