//! A home's gateway: the one node front ends pair with and dial, which
//! forwards each request to the back end that registered its application.
//!
//! Back ends dial out to the gateway, pair with it once at its door,
//! [`GATEWAY_APP`], and register the applications they serve there with
//! [`REGISTER_METHOD`]. The gateway serves those applications to front ends
//! as if it served them itself. It authorizes each request, opens a stream to
//! the back end on the back end's own connection, sends it the request's
//! header with the application, the caller's key and the authorizer's
//! claims, and then copies the stream both ways.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ciborium::Value;
use iroh::endpoint::{Connection, Incoming, RecvStream, SendStream, VarInt};
use iroh::protocol::ProtocolHandler;
use iroh::Endpoint;
use serde::Deserialize;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;

use crate::addr::PeerId;
use crate::hooks::{AuthRequest, Caller};
use crate::node::{refuse, AppHandler, Target};
use crate::pairing::{Pairing, PAIR_APP};
use crate::peers::{write_atomically, PeerSet};
use crate::wire::{write_frame, Fault, FaultCode, Header, WireError, WIRE_VERSION};

/// The application a gateway serves for its back ends (ALPN
/// `kuri-link/<v>/gateway`). A back end pairs there, redeeming a ticket from
/// [`Gateway::pairing`] with [`Node::pair_gateway`](crate::Node::pair_gateway),
/// and registers its applications there. Reserved: a gateway serves no
/// application of this name, and no back end can register one.
pub const GATEWAY_APP: &str = "gateway";

/// The method a back end opens on [`GATEWAY_APP`] to register the
/// applications it serves, with the arguments `{"apps": [name, …]}`. The
/// gateway answers, then sends its own address and each change of it, as for
/// [`ADDR_METHOD`](crate::ADDR_METHOD). The applications stay registered
/// while the stream is open.
pub const REGISTER_METHOD: &str = "kuri-link.register";

/// The longest application name a back end can register. The name goes into
/// an ALPN, which holds at most 255 bytes.
const MAX_APP_NAME: usize = 200;

/// The code a gateway closes a back end's connection with once the back end
/// is unpaired.
const UNPAIRED: u32 = 1;

/// The code a forwarded stream's half is reset with when the other half
/// failed.
const BROKEN: u32 = 2;

/// The code a gateway closes a back end's older connection with once the back
/// end registers on a new one.
const SUPERSEDED: u32 = 3;

/// A home's gateway: the back ends it admits, and which of them serves each
/// application.
///
/// Install it with [`NodeBuilder::gateway`](crate::NodeBuilder::gateway). A
/// back end pairs with it through [`Gateway::pairing`], then registers its
/// applications. An application's name belongs to the back end that first
/// registered it: the same back end registering again, say after a restart,
/// replaces its registration, and another back end is refused the name until
/// the first is unpaired.
///
/// Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Gateway {
    inner: Arc<Shared>,
}

struct Shared {
    back_ends: Pairing,
    state: Mutex<State>,
    /// Counts the changes to the registered names, so that the gateway's
    /// node accepts exactly their ALPNs.
    names: watch::Sender<u64>,
    next_registration: AtomicU64,
}

struct State {
    /// Which back end owns each application name.
    owners: BTreeMap<String, PeerId>,
    file: Option<PathBuf>,
    /// The registrations open now, by application.
    live: HashMap<String, Live>,
    /// The applications the gateway's own node serves.
    local: BTreeSet<String>,
}

/// One back end's registration of an application.
struct Live {
    back_end: PeerId,
    conn: Connection,
    /// Which registration this is, so that one ending removes only itself.
    id: u64,
}

/// An application that a gateway's back ends registered. More fields may
/// come.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GatewayApp {
    pub name: String,
    /// The back end the name belongs to.
    pub owner: PeerId,
    /// Whether that back end is registered now, so that front ends reach it.
    pub online: bool,
}

impl Gateway {
    /// A gateway that admits the back ends in `back_ends`, and keeps in
    /// memory which back end owns each application name.
    pub fn new(back_ends: PeerSet) -> Gateway {
        Gateway::with_owners(back_ends, BTreeMap::new(), None)
    }

    /// A gateway that keeps which back end owns each application name in the
    /// file at `path`, read now if the file exists, so that a restart keeps
    /// them. The file holds one line per application: the owner's key, a
    /// space, and the name. It is written as a [`PeerSet`]'s file is.
    pub fn open(back_ends: PeerSet, path: impl Into<PathBuf>) -> io::Result<Gateway> {
        let path = path.into();
        let owners = match std::fs::read_to_string(&path) {
            Ok(text) => parse_owners(&text)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Gateway::with_owners(back_ends, owners, Some(path)))
    }

    fn with_owners(
        back_ends: PeerSet,
        owners: BTreeMap<String, PeerId>,
        file: Option<PathBuf>,
    ) -> Gateway {
        Gateway {
            inner: Arc::new(Shared {
                back_ends: Pairing::with_door(back_ends, GATEWAY_APP),
                state: Mutex::new(State {
                    owners,
                    file,
                    live: HashMap::new(),
                    local: BTreeSet::new(),
                }),
                names: watch::Sender::new(0),
                next_registration: AtomicU64::new(0),
            }),
        }
    }

    /// Issues tickets for back ends. A back end that redeems one with
    /// [`Node::pair_gateway`](crate::Node::pair_gateway) joins the gateway's
    /// back ends, [`Pairing::peers`], and may register applications. A
    /// front end's ticket from this pairing admits it nowhere.
    ///
    /// To unpair a back end, remove it from [`Pairing::peers`]: its
    /// registration ends at once, and its application names are released.
    /// The gateway stops accepting them, as it does a name no back end
    /// registered, until a back end registers them again.
    pub fn pairing(&self) -> &Pairing {
        &self.inner.back_ends
    }

    /// The applications paired back ends registered, in order of name.
    pub fn apps(&self) -> Vec<GatewayApp> {
        let peers = self.inner.back_ends.peers();
        let state = self.state();
        state
            .owners
            .iter()
            .filter(|(_, owner)| peers.contains(owner))
            .map(|(name, owner)| GatewayApp {
                name: name.clone(),
                owner: *owner,
                online: state
                    .live
                    .get(name)
                    .is_some_and(|live| self.reaches(live) && live.back_end == *owner),
            })
            .collect()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        crate::node::lock(&self.inner.state)
    }

    /// Whether requests can go to the back end of `live`.
    fn reaches(&self, live: &Live) -> bool {
        live.conn.close_reason().is_none() && self.inner.back_ends.peers().contains(&live.back_end)
    }

    /// Keeps back ends from registering the applications the gateway's own
    /// node serves.
    pub(crate) fn serve_locally(&self, apps: impl Iterator<Item = String>) {
        self.state().local.extend(apps);
    }

    /// The application names that back ends registered.
    fn names(&self) -> Vec<String> {
        self.state().owners.keys().cloned().collect()
    }

    fn owns(&self, app: &str) -> bool {
        self.state().owners.contains_key(app)
    }

    /// Serves [`REGISTER_METHOD`] for `back_end`, which the door admitted, on
    /// its connection `conn`.
    pub(crate) async fn register(
        &self,
        door: &AppHandler,
        back_end: PeerId,
        conn: &Connection,
        args: Value,
        mut send: SendStream,
    ) -> Result<(), WireError> {
        let max = door.max_frame;
        let apps = match app_names(args) {
            Ok(apps) => apps,
            Err(fault) => return refuse(&mut send, fault, max).await,
        };
        // Claiming a new name writes the owners' file and syncs it; keep that
        // off the runtime.
        let gateway = self.clone();
        let registering = conn.clone();
        let claimed =
            tokio::task::spawn_blocking(move || gateway.claim(back_end, &apps, registering))
                .await
                .unwrap_or_else(|e| Err(Fault::new(FaultCode::App, e.to_string())));
        let id = match claimed {
            Ok(id) => id,
            Err(fault) => return refuse(&mut send, fault, max).await,
        };
        let request = AuthRequest {
            peer: back_end,
            app: GATEWAY_APP,
            method: REGISTER_METHOD,
            credential: None,
        };
        let served = door.send_addr(request, send, self.unpaired(back_end)).await;
        self.withdraw(id);
        tracing::info!(back_end = %back_end.fmt_short(), "a back end's registration ended");
        if !self.inner.back_ends.peers().contains(&back_end) {
            conn.close(VarInt::from_u32(UNPAIRED), b"unpaired from the gateway");
        }
        served
    }

    /// Records `back_end` as registered for `apps` on `conn`, and as their
    /// owner if it is not already. Returns which registration this is.
    fn claim(&self, back_end: PeerId, apps: &[String], conn: Connection) -> Result<u64, Fault> {
        let peers = self.inner.back_ends.peers();
        let mut state = self.state();
        // The door admitted `back_end`, but it may have been unpaired since and
        // its names released. `release_unpaired` takes the same lock, so either
        // this sees it gone, or the release that its unpairing sets off sees
        // the names claimed here.
        if !peers.contains(&back_end) {
            return Err(Fault::new(FaultCode::Denied, "unpaired from the gateway"));
        }
        for app in apps {
            if state.local.contains(app) {
                return Err(Fault::new(
                    FaultCode::Denied,
                    format!("the gateway serves {app:?} itself"),
                ));
            }
            match state.owners.get(app) {
                Some(owner) if *owner != back_end && peers.contains(owner) => {
                    return Err(Fault::new(
                        FaultCode::Denied,
                        format!(
                            "{app:?} belongs to the back end {}; unpair that one from \
                             the gateway first",
                            owner.fmt_short()
                        ),
                    ));
                }
                _ => {}
            }
        }
        let before = state.owners.clone();
        for app in apps {
            state.owners.insert(app.clone(), back_end);
        }
        if state.owners != before {
            if let Err(e) = state.persist() {
                state.owners = before;
                return Err(Fault::new(
                    FaultCode::App,
                    format!("storing who serves the applications: {e}"),
                ));
            }
        }
        let new_name = apps.iter().any(|app| !before.contains_key(app));
        // The same back end on another connection has restarted, or lost its
        // network, without the gateway hearing it go. Requests on the old
        // connection would wait for its idle timeout; closing it fails them now.
        let superseded: Vec<Connection> = state
            .live
            .values()
            .filter(|live| live.back_end == back_end && live.conn.stable_id() != conn.stable_id())
            .map(|live| live.conn.clone())
            .collect();
        let id = self.inner.next_registration.fetch_add(1, Ordering::Relaxed);
        for app in apps {
            let live = Live {
                back_end,
                conn: conn.clone(),
                id,
            };
            state.live.insert(app.clone(), live);
        }
        drop(state);
        for old in superseded {
            old.close(
                VarInt::from_u32(SUPERSEDED),
                b"the back end registered again",
            );
        }
        if new_name {
            self.inner.names.send_modify(|count| *count += 1);
        }
        tracing::info!(back_end = %back_end.fmt_short(), ?apps, "a back end registered");
        Ok(id)
    }

    /// Ends registration `id`, unless a later one replaced it already.
    fn withdraw(&self, id: u64) {
        self.state().live.retain(|_, live| live.id != id);
    }

    /// Forgets the names of back ends that are no longer paired. Writes the
    /// owners' file, so call it on a blocking thread.
    fn release_unpaired(&self) {
        let peers = self.inner.back_ends.peers();
        let mut state = self.state();
        let before = state.owners.len();
        state.owners.retain(|_, owner| peers.contains(owner));
        if state.owners.len() == before {
            return;
        }
        // The names stay released even if the file keeps them: the next
        // start releases them again.
        if let Err(e) = state.persist() {
            tracing::warn!(error = %e, "storing who serves the applications");
        }
        drop(state);
        self.inner.names.send_modify(|count| *count += 1);
        tracing::info!("released the names of unpaired back ends");
    }

    /// Finishes once `back_end` is no longer paired.
    async fn unpaired(&self, back_end: PeerId) {
        let peers = self.inner.back_ends.peers();
        let mut changes = peers.changes();
        while peers.contains(&back_end) {
            if changes.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    /// The connection to the back end registered for `app`, if it can take
    /// requests.
    fn back_end(&self, app: &str) -> Option<Connection> {
        let state = self.state();
        let live = state.live.get(app)?;
        self.reaches(live).then(|| live.conn.clone())
    }

    /// Forwards a request for `app`, which `caller` was authorized for, to
    /// the application's back end, or answers that none serves it now.
    pub(crate) async fn forward(
        &self,
        app: &str,
        caller: Caller,
        mut header: Header,
        mut send: SendStream,
        mut recv: RecvStream,
        max: usize,
    ) -> Result<(), WireError> {
        let unavailable = || {
            Fault::new(
                FaultCode::Unavailable,
                format!("no back end serves {app} at this gateway now"),
            )
        };
        let Some(phone) = caller.peer() else {
            return refuse(&mut send, unavailable(), max).await;
        };
        let opened = match self.back_end(app) {
            Some(conn) => conn.open_bi().await.ok(),
            None => None,
        };
        let Some((mut to_send, mut to_recv)) = opened else {
            return refuse(&mut send, unavailable(), max).await;
        };
        header.app = Some(app.to_owned());
        header.caller = Some(serde_bytes::ByteBuf::from(phone.as_bytes().to_vec()));
        header.claims = Some(caller.claims).filter(|claims| *claims != Value::Null);
        if write_frame(&mut to_send, &header, max).await.is_err() {
            let _ = to_send.reset(VarInt::from_u32(BROKEN));
            return refuse(&mut send, unavailable(), max).await;
        }
        let (up, down) =
            tokio::join!(pipe(&mut recv, &mut to_send), pipe(&mut to_recv, &mut send),);
        up.and(down)
    }
}

impl fmt::Debug for Gateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        f.debug_struct("Gateway")
            .field("back_ends", &self.inner.back_ends)
            .field("apps", &state.owners.len())
            .field("registered", &state.live.len())
            .field("file", &state.file)
            .finish()
    }
}

impl State {
    fn persist(&self) -> io::Result<()> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        write_atomically(path, owners_text(&self.owners).as_bytes())
    }
}

fn owners_text(owners: &BTreeMap<String, PeerId>) -> String {
    owners
        .iter()
        .map(|(app, owner)| format!("{owner} {app}\n"))
        .collect()
}

fn parse_owners(text: &str) -> io::Result<BTreeMap<String, PeerId>> {
    let invalid = |line: &str, why: String| {
        io::Error::new(io::ErrorKind::InvalidData, format!("line {line:?}: {why}"))
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, app) = line
                .split_once(' ')
                .ok_or_else(|| invalid(line, "expected a key and an application".into()))?;
            let owner = PeerId::from_str(key).map_err(|e| invalid(line, e.to_string()))?;
            Ok((app.trim().to_owned(), owner))
        })
        .collect()
}

/// The arguments of [`REGISTER_METHOD`].
#[derive(Deserialize)]
struct RegisterArgs {
    apps: Vec<String>,
}

/// The application names a registration lists, each once, or the fault that
/// refuses it.
fn app_names(args: Value) -> Result<Vec<String>, Fault> {
    let bad = |why: String| Fault::new(FaultCode::BadRequest, why);
    let args: RegisterArgs = args
        .deserialized()
        .map_err(|e| bad(format!("registering: {e}")))?;
    let apps: BTreeSet<String> = args.apps.into_iter().collect();
    if apps.is_empty() {
        return Err(bad("registering no application".into()));
    }
    for app in &apps {
        if app.is_empty()
            || app.len() > MAX_APP_NAME
            || app.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(bad(format!(
                "{app:?} is not an application name: 1 to {MAX_APP_NAME} bytes, \
                 without spaces or control characters"
            )));
        }
        if app == GATEWAY_APP || app == PAIR_APP {
            return Err(bad(format!("the application name {app:?} is reserved")));
        }
    }
    Ok(apps.into_iter().collect())
}

/// Copies `from` into `to` until `from` finishes, then finishes `to`. If
/// either side fails, resets `to` and stops `from`, so that the failure
/// reaches both ends instead of reading as a clean end.
async fn pipe(from: &mut RecvStream, to: &mut SendStream) -> Result<(), WireError> {
    match tokio::io::copy(from, to).await {
        Ok(_) => to.finish().map_err(|e| WireError::Stream(e.to_string())),
        Err(e) => {
            let _ = to.reset(VarInt::from_u32(BROKEN));
            let _ = from.stop(VarInt::from_u32(BROKEN));
            Err(WireError::Stream(e.to_string()))
        }
    }
}

/// Accepts a gateway's connections and hands each to its application's
/// handler. It takes the place of iroh's router, whose applications are fixed
/// when it starts, because a gateway's grow as back ends register new ones.
pub(crate) struct Acceptor {
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Acceptor {
    /// Accepts on `endpoint` the applications of `handlers`, and each that
    /// `gateway`'s back ends registered, through `forward`.
    pub(crate) fn spawn(
        endpoint: Endpoint,
        gateway: Gateway,
        handlers: Vec<AppHandler>,
        forward: AppHandler,
    ) -> Acceptor {
        let handlers = handlers
            .into_iter()
            .map(|handler| (crate::alpn(&handler.app), handler))
            .collect();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(accept(
            endpoint,
            gateway,
            Arc::new(handlers),
            forward,
            stopped,
        ));
        Acceptor {
            stop: Some(stop),
            task,
        }
    }

    /// Stops accepting and closes the endpoint.
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

impl Drop for Acceptor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl fmt::Debug for Acceptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Acceptor")
    }
}

async fn accept(
    endpoint: Endpoint,
    gateway: Gateway,
    handlers: Arc<HashMap<Vec<u8>, AppHandler>>,
    forward: AppHandler,
    mut stopped: oneshot::Receiver<()>,
) {
    let alpns = || {
        let mut alpns: Vec<Vec<u8>> = handlers.keys().cloned().collect();
        alpns.extend(gateway.names().iter().map(|app| crate::alpn(app)));
        alpns
    };
    let release = || {
        let gateway = gateway.clone();
        tokio::task::spawn_blocking(move || gateway.release_unpaired())
    };
    let mut names = gateway.inner.names.subscribe();
    let mut back_ends = gateway.inner.back_ends.peers().changes();
    // A back end unpaired while the gateway was down.
    let _ = release().await;
    endpoint.set_alpns(alpns());
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            Ok(()) = names.changed() => endpoint.set_alpns(alpns()),
            Ok(()) = back_ends.changed() => {
                let _ = release().await;
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                connections.spawn(connection(
                    incoming,
                    handlers.clone(),
                    gateway.clone(),
                    forward.clone(),
                ));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    connections.abort_all();
    endpoint.close().await;
}

async fn connection(
    incoming: Incoming,
    handlers: Arc<HashMap<Vec<u8>, AppHandler>>,
    gateway: Gateway,
    forward: AppHandler,
) {
    let Ok(mut accepting) = incoming.accept() else {
        return;
    };
    let Ok(alpn) = accepting.alpn().await else {
        return;
    };
    let Ok(conn) = accepting.await else {
        return;
    };
    let prefix = format!("kuri-link/{WIRE_VERSION}/");
    let registered = std::str::from_utf8(&alpn)
        .ok()
        .and_then(|alpn| alpn.strip_prefix(&prefix))
        .filter(|app| gateway.owns(app));
    let handler = match (handlers.get(&alpn), registered) {
        (Some(handler), _) => handler.clone(),
        (None, Some(app)) => AppHandler {
            app: app.into(),
            target: Target::Forward(gateway.clone()),
            ..forward
        },
        (None, None) => {
            conn.close(VarInt::from_u32(0), b"no such application");
            return;
        }
    };
    if let Err(e) = handler.accept(conn).await {
        tracing::debug!(app = %handler.app, error = %e, "a connection ended");
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration;

    use iroh::endpoint::presets;
    use iroh::RelayMode;

    use super::*;
    use crate::hooks::{Authorizer, Denied, Origin};
    use crate::wire::{read_frame, Op, Reply, DEFAULT_MAX_FRAME};
    use crate::{BoxFuture, Methods, Node, NodeKey, PeerAddr, Registration, Relays};

    const APP: &str = "camera";

    fn loopback() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
    }

    /// `whoami`: the caller's key and claims, as the service sees them.
    fn whoami() -> Methods {
        Methods::new().unary("whoami", |caller, _args| async move {
            let key = caller.peer().map(|peer| Value::Text(peer.to_string()));
            Ok(Value::Array(vec![
                key.unwrap_or(Value::Null),
                caller.claims,
            ]))
        })
    }

    /// Admits anyone, and says so in its claims.
    struct Vouching;

    impl Authorizer for Vouching {
        fn authorize<'a>(&'a self, req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>> {
            Box::pin(async move {
                Ok(Caller {
                    origin: Origin::Peer(req.peer),
                    claims: Value::Text("the authorizer's".into()),
                })
            })
        }
    }

    /// Dials [`APP`] at `at` as `key`, over loopback.
    async fn dial(key: NodeKey, at: PeerAddr) -> (Endpoint, Connection) {
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(key.into_iroh())
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr(loopback())
            .unwrap()
            .bind()
            .await
            .unwrap();
        let conn = endpoint
            .connect(at.to_iroh(), &crate::alpn(APP))
            .await
            .unwrap();
        (endpoint, conn)
    }

    /// Calls `whoami` at `at` as `key`, with a header that names another
    /// caller, another application and claims of the front end's own making,
    /// as only a gateway's should.
    async fn whoami_naming_another(key: NodeKey, at: PeerAddr) -> Value {
        let (endpoint, conn) = dial(key, at).await;
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let mut header = Header::new(Op::Call, "whoami", Value::Null, None);
        header.app = Some("media".into());
        let another = NodeKey::generate().public();
        header.caller = Some(serde_bytes::ByteBuf::from(another.as_bytes().to_vec()));
        header.claims = Some(Value::Text("the front end's own".into()));
        write_frame(&mut send, &header, DEFAULT_MAX_FRAME)
            .await
            .unwrap();
        send.finish().unwrap();
        let reply = read_frame::<Reply>(&mut recv, DEFAULT_MAX_FRAME)
            .await
            .unwrap();
        endpoint.close().await;
        match reply {
            Reply::Ok(seen) => seen,
            Reply::Err(fault) => panic!("{fault}"),
        }
    }

    fn seen(caller: PeerId, claims: Value) -> Value {
        Value::Array(vec![Value::Text(caller.to_string()), claims])
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_front_end_cannot_speak_for_another_to_a_back_end_it_dials() {
        let camera = Node::builder()
            .relays(Relays::None)
            .bind_addr(loopback())
            .serve(APP, whoami())
            .authorizer(Vouching)
            .bind()
            .await
            .unwrap();
        let phone = NodeKey::generate();
        let id = phone.public();
        assert_eq!(
            whoami_naming_another(phone, camera.bound_addr()).await,
            seen(id, Value::Text("the authorizer's".into()))
        );
        camera.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_front_end_cannot_speak_for_another_through_a_gateway() {
        let phones = PeerSet::new();
        let gateway = Gateway::new(PeerSet::new());
        let home = Node::builder()
            .relays(Relays::None)
            .bind_addr(loopback())
            .pairing(Pairing::new(phones.clone()))
            .gateway(gateway.clone())
            .bind()
            .await
            .unwrap();
        let camera = Node::builder()
            .relays(Relays::None)
            .bind_addr(loopback())
            .serve(APP, whoami())
            .behind_gateway(home.bound_addr())
            .bind()
            .await
            .unwrap();
        let ticket = gateway
            .pairing()
            .issue(home.bound_addr(), Duration::from_secs(60));
        camera.pair_gateway(&ticket).await.unwrap();
        let mut registration = camera.registration().unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            registration.wait_for(|r| *r == Registration::Registered),
        )
        .await
        .expect("registers in time")
        .unwrap();

        let phone = NodeKey::generate();
        let id = phone.public();
        phones.insert(id).unwrap();
        // The gateway's authorizer said nothing of the phone, so the back
        // end hears nothing either.
        assert_eq!(
            whoami_naming_another(phone, home.bound_addr()).await,
            seen(id, Value::Null)
        );
        camera.shutdown().await;
        home.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_back_end_unpaired_before_it_claims_claims_nothing() {
        let camera = Node::builder()
            .relays(Relays::None)
            .bind_addr(loopback())
            .serve(APP, whoami())
            .bind()
            .await
            .unwrap();
        let back_ends = PeerSet::new();
        let gateway = Gateway::new(back_ends.clone());
        let key = NodeKey::generate();
        let back_end = key.public();
        let (endpoint, conn) = dial(key, camera.bound_addr()).await;
        let apps = [APP.to_owned()];

        // The door admitted it, then it was unpaired before its claim ran.
        back_ends.insert(back_end).unwrap();
        back_ends.remove(&back_end).unwrap();
        let fault = gateway.claim(back_end, &apps, conn.clone()).unwrap_err();
        assert_eq!(fault.code, FaultCode::Denied, "{fault}");
        assert!(gateway.names().is_empty());

        back_ends.insert(back_end).unwrap();
        gateway.claim(back_end, &apps, conn).unwrap();
        assert_eq!(gateway.names(), apps);
        endpoint.close().await;
        camera.shutdown().await;
    }

    fn args(apps: &[&str]) -> Value {
        Value::Map(vec![(
            Value::Text("apps".into()),
            Value::Array(apps.iter().map(|app| Value::Text((*app).into())).collect()),
        )])
    }

    #[test]
    fn a_registration_names_applications_a_gateway_can_serve() {
        assert_eq!(
            app_names(args(&["media", "camera", "media"])).unwrap(),
            vec!["camera".to_owned(), "media".to_owned()]
        );
        for refused in [
            &[][..],
            &[""],
            &["two words"],
            &["line\nbreak"],
            &[GATEWAY_APP],
            &[PAIR_APP],
        ] {
            let fault = app_names(args(refused)).unwrap_err();
            assert_eq!(fault.code, FaultCode::BadRequest, "{refused:?}: {fault}");
        }
        let long = "a".repeat(MAX_APP_NAME + 1);
        assert!(app_names(args(&[&long])).is_err());
        assert!(app_names(Value::Null).is_err());
    }

    #[test]
    fn the_owners_file_reads_back_what_it_wrote() {
        let owner = crate::NodeKey::from_bytes(&[3; 32]).public();
        let owners: BTreeMap<String, PeerId> =
            [("camera".to_owned(), owner), ("media".to_owned(), owner)].into();
        assert_eq!(parse_owners(&owners_text(&owners)).unwrap(), owners);
        assert_eq!(
            parse_owners(&format!("# by hand\n\n{owner} camera\n")).unwrap(),
            [("camera".to_owned(), owner)].into()
        );
        assert!(parse_owners("camera\n").is_err());
        assert!(parse_owners("not-a-key camera\n").is_err());
    }
}
