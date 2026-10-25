//! A back end's side of a gateway: the connection it dials out, the
//! applications it registers there, and the requests the gateway forwards.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ciborium::Value;
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream, VarInt};
use iroh::Endpoint;
use tokio::sync::{watch, Notify};

use crate::addr::{AddrWire, PeerAddr, PeerId};
use crate::gateway::{GATEWAY_APP, REGISTER_METHOD};
use crate::hooks::{AuthRequest, Authorizer, Caller, Origin};
use crate::node::{dial, dispatch, fault_of, read_header, refuse, RESERVED_PREFIX};
use crate::service::{AppError, Service};
use crate::wire::{read_frame, write_frame, Fault, FaultCode, Header, Op, Reply, WireError};

/// The first wait before dialing the gateway again, doubled after each
/// failure up to [`LONGEST_WAIT`].
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// How long a registration must hold for the waits to start over. Shorter
/// ones count as failures, so that two back ends with one key, each taking
/// the registration from the other, do so ever more slowly: the gateway
/// closes the connection of the one it took the registration from. One whose
/// connection went silent instead, as when the gateway crashed or the path to
/// it dropped out, starts the waits over however short it held.
const HELD: Duration = Duration::from_secs(30);

/// How long the gateway or a back end waits to hear from the other before it
/// takes the connection for lost. A back end's own timeout bounds both ends',
/// so that a back end gone without a word stops taking requests within this,
/// instead of after the 30 seconds that suit a phone on a weak network.
///
/// A gateway that crashed and came back is noticed the same way, by the back
/// end: the new gateway does not know the old connection. QUIC restarts the
/// timer once more when the back end sends its next keep-alive, so the back
/// end notices within [`KEEP_ALIVE`] and this, and registers again a wait
/// later.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(4);

/// How often a back end, with nothing else to send, tells its gateway that it
/// is there. Both run all the time and mostly share a network, so this can be
/// far more often than iroh's 5 seconds, which suit a phone's battery.
pub(crate) const KEEP_ALIVE: Duration = Duration::from_secs(1);

/// Where a back end behind a gateway stands with it. More cases may come, so
/// a `match` on it needs a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Registration {
    /// Dialing the gateway, or waiting to dial it again because the gateway
    /// could not be reached or the connection to it was lost.
    Connecting,
    /// The gateway forwards requests for this node's applications to it.
    Registered,
    /// The gateway refused the registration: this node has not paired with
    /// it, or another back end owns one of its applications' names. The node
    /// tries again, as it does when the gateway cannot be reached.
    Refused(Fault),
}

/// A back end's link to its gateway, kept up by a task of its own until
/// dropped.
pub(crate) struct Uplink {
    gateway: watch::Receiver<PeerAddr>,
    registration: watch::Receiver<Registration>,
    /// Cuts the wait before dialing again short, as after pairing.
    retry: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Uplink {
    pub(crate) fn spawn(
        endpoint: Endpoint,
        gateway: PeerAddr,
        apps: Vec<(String, Arc<dyn Service>)>,
        authorizer: Option<Arc<dyn Authorizer>>,
        max_frame: usize,
    ) -> Uplink {
        let (gateway_tx, gateway) = watch::channel(gateway);
        let (registration_tx, registration) = watch::channel(Registration::Connecting);
        let retry = Arc::new(Notify::new());
        let back_end = Arc::new(BackEnd {
            apps: apps.into_iter().collect(),
            authorizer,
            max_frame,
        });
        let task = tokio::spawn(run(
            endpoint,
            back_end,
            gateway_tx,
            registration_tx,
            retry.clone(),
        ));
        Uplink {
            gateway,
            registration,
            retry,
            task,
        }
    }

    pub(crate) fn registration(&self) -> watch::Receiver<Registration> {
        self.registration.clone()
    }

    pub(crate) fn gateway_addr(&self) -> watch::Receiver<PeerAddr> {
        self.gateway.clone()
    }

    /// Dials the gateway again now, if the link is waiting to. Kept for no
    /// later wait: one that comes after a registration must run its course.
    pub(crate) fn retry(&self) {
        self.retry.notify_waiters();
    }
}

impl Drop for Uplink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for Uplink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Uplink")
            .field("gateway", &self.gateway.borrow().id.fmt_short())
            .field("registration", &*self.registration.borrow())
            .finish()
    }
}

/// The applications a back end serves through its gateway.
struct BackEnd {
    apps: HashMap<String, Arc<dyn Service>>,
    authorizer: Option<Arc<dyn Authorizer>>,
    max_frame: usize,
}

/// Dials the gateway, registers, and serves what it forwards, again and
/// again, for as long as the node runs.
async fn run(
    endpoint: Endpoint,
    back_end: Arc<BackEnd>,
    gateway: watch::Sender<PeerAddr>,
    registration: watch::Sender<Registration>,
    retry: Arc<Notify>,
) {
    let mut wait = FIRST_WAIT;
    loop {
        let addr = gateway.borrow().clone();
        let ended = match dial(&endpoint, &addr, GATEWAY_APP, None).await {
            Ok(conn) => {
                let ended = back_end.clone().serve(&conn, &gateway, &registration).await;
                conn.close(VarInt::from_u32(0), b"");
                ended
            }
            Err(e) => Err(Ended::Unreachable(e.to_string())),
        };
        match ended {
            Ok(held) => {
                if held.silent || held.lasted >= HELD {
                    wait = FIRST_WAIT;
                }
                registration.send_replace(Registration::Connecting);
                tracing::info!("the connection to the gateway ended");
            }
            Err(Ended::Unreachable(why)) => {
                registration.send_replace(Registration::Connecting);
                tracing::info!(error = %why, "the gateway cannot be reached");
            }
            Err(Ended::Refused(fault)) => {
                tracing::info!(%fault, "the gateway refused the registration");
                registration.send_replace(Registration::Refused(fault));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = retry.notified() => {}
        }
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

/// Why a connection to the gateway served nothing.
enum Ended {
    Unreachable(String),
    Refused(Fault),
}

/// How a registration ended: after how long, and whether its connection went
/// silent rather than the gateway ending it.
struct Held {
    lasted: Duration,
    silent: bool,
}

impl BackEnd {
    /// Registers on `conn` and serves the requests the gateway forwards there,
    /// until the connection or the registration ends. Returns how it ended.
    async fn serve(
        self: Arc<Self>,
        conn: &Connection,
        gateway: &watch::Sender<PeerAddr>,
        registration: &watch::Sender<Registration>,
    ) -> Result<Held, Ended> {
        let max = self.max_frame;
        let unreachable = |e: WireError| Ended::Unreachable(e.to_string());
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| Ended::Unreachable(e.to_string()))?;
        let apps = Value::Array(
            self.apps
                .keys()
                .map(|app| Value::Text(app.clone()))
                .collect(),
        );
        let args = Value::Map(vec![(Value::Text("apps".into()), apps)]);
        let header = Header::new(Op::Open, REGISTER_METHOD, args, None);
        write_frame(&mut send, &header, max)
            .await
            .map_err(unreachable)?;
        send.finish()
            .map_err(|e| Ended::Unreachable(e.to_string()))?;
        if let Reply::Err(fault) = read_frame::<Reply>(&mut recv, max)
            .await
            .map_err(unreachable)?
        {
            return Err(Ended::Refused(fault));
        }
        registration.send_replace(Registration::Registered);
        tracing::info!(gateway = %conn.remote_id().fmt_short(), "registered with the gateway");
        let since = tokio::time::Instant::now();

        let id = gateway.borrow().id;
        let addresses = async {
            while let Ok(update) = read_frame::<AddrWire>(&mut recv, max).await {
                match update.to_addr() {
                    Ok(addr) => learn(gateway, id, addr),
                    Err(e) => tracing::debug!(error = %e, "an address update that does not read"),
                }
            }
        };
        let requests = async {
            while let Ok((send, recv)) = conn.accept_bi().await {
                let this = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = this.serve_forwarded(send, recv).await {
                        tracing::debug!(error = %e, "a forwarded request failed");
                    }
                });
            }
        };
        // The gateway ends the registration by finishing its stream, as when
        // this back end is unpaired, and the connection with it.
        tokio::select! {
            _ = addresses => {}
            _ = requests => {}
        }
        // Silence ends the connection with a timeout, or with a reset if the
        // path comes back after the gateway gave the connection up and
        // before this end did: the gateway's idle timer runs from the last
        // packet it heard, and this end's restarts once more at the first
        // keep-alive it sends after that, up to a second later. A gateway
        // that ended the registration closes the connection instead.
        Ok(Held {
            lasted: since.elapsed(),
            silent: matches!(
                conn.close_reason(),
                Some(ConnectionError::TimedOut | ConnectionError::Reset)
            ),
        })
    }

    /// Answers one request the gateway forwarded.
    async fn serve_forwarded(
        self: Arc<Self>,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<(), WireError> {
        let max = self.max_frame;
        let Some(mut header) = read_header(&mut send, &mut recv, max).await? else {
            return Ok(());
        };
        let caller = header
            .caller
            .as_deref()
            .and_then(|bytes| <&[u8; 32]>::try_from(bytes.as_slice()).ok())
            .and_then(|bytes| PeerId::from_bytes(bytes).ok());
        let (Some(app), Some(caller)) = (header.app.clone(), caller) else {
            let fault = Fault::new(
                FaultCode::BadRequest,
                "a forwarded request names its application and its caller",
            );
            return refuse(&mut send, fault, max).await;
        };
        let Some(service) = self.apps.get(&app).cloned() else {
            let fault = Fault::new(
                FaultCode::Unavailable,
                format!("this back end does not serve {app}"),
            );
            return refuse(&mut send, fault, max).await;
        };
        if header.method.starts_with(RESERVED_PREFIX) {
            let fault = fault_of(AppError::NoSuchMethod(header.method));
            return refuse(&mut send, fault, max).await;
        }
        let claims = header.claims.take().unwrap_or(Value::Null);
        let caller = match &self.authorizer {
            None => Caller {
                origin: Origin::Peer(caller),
                claims,
            },
            Some(authorizer) => {
                let request = AuthRequest {
                    peer: caller,
                    app: &app,
                    method: &header.method,
                    credential: header.cred.as_deref().map(|c| c.as_slice()),
                };
                match authorizer.authorize(request).await {
                    Ok(caller) => caller,
                    Err(denied) => {
                        let fault = Fault::new(FaultCode::Denied, denied.0);
                        return refuse(&mut send, fault, max).await;
                    }
                }
            }
        };
        dispatch(service, caller, header, send, recv, max).await
    }
}

/// Takes `addr` as the gateway's address, if it is one.
fn learn(gateway: &watch::Sender<PeerAddr>, id: PeerId, addr: PeerAddr) {
    // An empty address, say in the middle of a network change, says nothing
    // about where the gateway will be.
    if addr.id != id || (addr.addrs.is_empty() && addr.relays.is_empty()) {
        return;
    }
    gateway.send_if_modified(|known| {
        if *known == addr {
            return false;
        }
        tracing::debug!(?addr, "the gateway's address changed");
        *known = addr;
        true
    });
}
