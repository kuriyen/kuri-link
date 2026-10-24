//! Pairing: one-time tickets that turn into lasting entries in a [`PeerSet`].
//!
//! A back end issues a [`Ticket`]: its address and a one-time token that
//! expires. A front end gets the ticket out of band, usually by scanning a QR
//! code, dials the address and calls `pair` with the token. The back end
//! spends the token and admits the key the handshake proved. From then on that
//! key is admitted like any paired peer, and the ticket is worthless.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ciborium::Value;
use serde::{Deserialize, Serialize};

use crate::addr::PeerAddr;
use crate::gateway::GATEWAY_APP;
use crate::hooks::{AuthRequest, Authorizer, Caller, Denied, Origin};
use crate::node::{CallError, Error, Node};
use crate::peers::PeerSet;
use crate::service::{AppError, Service};
use crate::BoxFuture;

/// The application a node with pairing serves for redeeming tickets (ALPN
/// `kuri-link/<v>/pair`). Reserved: such a node refuses to bind if it is also
/// asked to serve an application of this name.
pub const PAIR_APP: &str = "pair";

/// The one method of [`PAIR_APP`].
pub const PAIR_METHOD: &str = "pair";

const TICKET_PREFIX: &str = "KURI-LINK:";
const TICKET_VERSION: u32 = 1;

/// The longest a ticket stays redeemable. A ticket is meant to be shown and
/// scanned on the spot; one that lingers is a standing invitation.
const MAX_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// A one-time pairing secret: 128 random bits. `Debug` never shows them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PairToken([u8; 16]);

impl PairToken {
    fn generate() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the operating system's random source failed");
        PairToken(bytes)
    }

    /// The bytes a front end presents as its credential when redeeming.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for PairToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairToken(..)")
    }
}

/// What a pairing QR code carries: where the back end is, and a one-time token.
///
/// The text form is `KURI-LINK:` followed by base32, characters a QR code's
/// alphanumeric mode encodes compactly. Reading it back accepts lower case and
/// ignores whitespace. Until it is redeemed or expires, the text is a secret:
/// whoever presents it first can pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub addr: PeerAddr,
    pub token: PairToken,
}

/// The CBOR inside a ticket's text. Named keys, like the wire format, so a
/// later version can add fields that this one ignores.
#[derive(Serialize, Deserialize)]
struct TicketWire {
    v: u32,
    id: serde_bytes::ByteBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ip: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    relay: Vec<String>,
    token: serde_bytes::ByteBuf,
}

/// Why a ticket's text could not be read.
#[derive(Debug, Clone, thiserror::Error)]
#[error("not a pairing ticket: {0}")]
pub struct TicketError(String);

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let wire = TicketWire {
            v: TICKET_VERSION,
            id: serde_bytes::ByteBuf::from(self.addr.id.as_bytes().to_vec()),
            ip: self.addr.addrs.iter().map(|a| a.to_string()).collect(),
            relay: self.addr.relays.iter().map(|u| u.to_string()).collect(),
            token: serde_bytes::ByteBuf::from(self.token.0.to_vec()),
        };
        let mut cbor = Vec::new();
        ciborium::into_writer(&wire, &mut cbor).map_err(|_| fmt::Error)?;
        let text = data_encoding::BASE32_NOPAD.encode(&cbor);
        write!(f, "{TICKET_PREFIX}{text}")
    }
}

impl FromStr for Ticket {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let body = text
            .strip_prefix(TICKET_PREFIX)
            .ok_or_else(|| TicketError(format!("it does not start with {TICKET_PREFIX}")))?;
        let cbor = data_encoding::BASE32_NOPAD
            .decode(body.as_bytes())
            .map_err(|e| TicketError(format!("base32: {e}")))?;
        let wire: TicketWire = ciborium::from_reader(cbor.as_slice())
            .map_err(|e| TicketError(format!("cbor: {e}")))?;
        if wire.v != TICKET_VERSION {
            return Err(TicketError(format!(
                "version {} (this build reads {TICKET_VERSION})",
                wire.v
            )));
        }
        let addr = PeerAddr::from_parts(&wire.id, &wire.ip, &wire.relay)
            .map_err(|e| TicketError(e.to_string()))?;
        let token: [u8; 16] = wire
            .token
            .as_slice()
            .try_into()
            .map_err(|_| TicketError("the token is not 16 bytes".into()))?;
        Ok(Ticket {
            addr,
            token: PairToken(token),
        })
    }
}

/// Issues pairing tickets and turns redeemed ones into entries in a [`PeerSet`].
///
/// Install it with [`NodeBuilder::pairing`](crate::NodeBuilder::pairing). The
/// node then serves [`PAIR_APP`], whose one method adds the caller's proven
/// key to the set. A live ticket's token admits its holder to that method and
/// nothing else, and is spent the moment it is presented there, whether or
/// not the pairing then succeeds, so a ticket admits one device at most.
///
/// Tickets live only in memory: a restart withdraws every one. Cheap to
/// clone; clones share tickets and peers.
#[derive(Clone)]
pub struct Pairing {
    peers: PeerSet,
    tokens: Arc<Mutex<HashMap<PairToken, Instant>>>,
    /// The application whose method `pair` redeems a ticket: [`PAIR_APP`],
    /// or a gateway's own door for back ends.
    door: &'static str,
}

impl Pairing {
    /// Pairing that adds each redeemed ticket's holder to `peers`.
    pub fn new(peers: PeerSet) -> Self {
        Pairing::with_door(peers, PAIR_APP)
    }

    pub(crate) fn with_door(peers: PeerSet, door: &'static str) -> Self {
        Pairing {
            peers,
            tokens: Arc::default(),
            door,
        }
    }

    /// The paired peers: what pairing adds to, and what to remove a device
    /// from to unpair it.
    pub fn peers(&self) -> &PeerSet {
        &self.peers
    }

    /// Issues a ticket for the back end at `addr`, usually [`Node::addr`],
    /// good for one device within `ttl`. `ttl` is capped at a day.
    pub fn issue(&self, addr: PeerAddr, ttl: Duration) -> Ticket {
        let token = PairToken::generate();
        let now = Instant::now();
        let mut tokens = self.tokens();
        tokens.retain(|_, expires| *expires > now);
        tokens.insert(token, now + ttl.min(MAX_TTL));
        Ticket { addr, token }
    }

    /// Withdraws `ticket`. Returns whether it could still have been redeemed.
    pub fn revoke(&self, ticket: &Ticket) -> bool {
        let removed = self.tokens().remove(&ticket.token);
        removed.is_some_and(|expires| expires > Instant::now())
    }

    /// Withdraws every ticket not yet redeemed.
    pub fn revoke_all(&self) {
        self.tokens().clear();
    }

    /// Spends `token`. Returns whether it was live.
    fn redeem(&self, token: &[u8]) -> bool {
        let Ok(token) = <[u8; 16]>::try_from(token) else {
            return false;
        };
        let removed = self.tokens().remove(&PairToken(token));
        removed.is_some_and(|expires| expires > Instant::now())
    }

    fn tokens(&self) -> MutexGuard<'_, HashMap<PairToken, Instant>> {
        self.tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The service behind [`PAIR_APP`].
    pub(crate) fn service(&self) -> PairService {
        PairService {
            peers: self.peers.clone(),
        }
    }
}

impl fmt::Debug for Pairing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pairing")
            .field("peers", &self.peers)
            .field("outstanding_tickets", &self.tokens().len())
            .finish()
    }
}

impl Authorizer for Pairing {
    fn authorize<'a>(&'a self, req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>> {
        // Presented at the pairing door, a token is spent whatever happens
        // next, so one ticket can never admit a second device.
        let redeemed = req.app == self.door
            && req.method == PAIR_METHOD
            && req.credential.is_some_and(|token| self.redeem(token));
        let admitted = redeemed || self.peers.contains(&req.peer);
        Box::pin(async move {
            if admitted {
                Ok(Caller {
                    origin: Origin::Peer(req.peer),
                    claims: Value::Null,
                })
            } else {
                Err(Denied(format!(
                    "{} is not paired with this node",
                    req.peer.fmt_short()
                )))
            }
        })
    }
}

/// Adds the caller to the peer set. Reached only by a caller the pairing
/// admitted: already paired, or holding a live token it has just spent.
pub(crate) struct PairService {
    peers: PeerSet,
}

impl Service for PairService {
    fn call(
        self: Arc<Self>,
        caller: Caller,
        method: String,
        _args: Value,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        Box::pin(async move {
            if method != PAIR_METHOD {
                return Err(AppError::NoSuchMethod(method));
            }
            let peer = caller
                .peer()
                .ok_or_else(|| AppError::Failed("only a peer can pair".into()))?;
            // The set may write a file and sync it; keep that off the runtime.
            let peers = self.peers.clone();
            let stored = tokio::task::spawn_blocking(move || peers.insert(peer))
                .await
                .map_err(|e| e.to_string())
                .and_then(|inserted| inserted.map_err(|e| e.to_string()));
            match stored {
                Ok(_) => Ok(Value::Null),
                Err(e) => Err(AppError::Failed(format!("storing the pairing: {e}"))),
            }
        })
    }
}

/// Why pairing with a back end failed. More cases may come, so a `match` on
/// it needs a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PairError {
    /// The back end could not be reached.
    #[error(transparent)]
    Connect(#[from] Error),
    /// The back end refused the ticket (spent, expired, withdrawn, or never
    /// issued: [`FaultCode::Denied`](crate::FaultCode::Denied)), or the
    /// request failed on the way.
    #[error("pairing: {0}")]
    Call(#[from] CallError),
}

impl Node {
    /// Redeems `ticket` with the back end that issued it, which from then on
    /// admits this node's key. Returns the back end's address, to keep and
    /// pass to [`Node::connect`] later.
    pub async fn pair(&self, ticket: &Ticket) -> Result<PeerAddr, PairError> {
        self.redeem(ticket, PAIR_APP).await
    }

    /// Redeems `ticket`, which a gateway's [`Gateway::pairing`] issued, so
    /// that the gateway admits this node as a back end. Returns the gateway's
    /// address, to keep and pass to
    /// [`NodeBuilder::behind_gateway`](crate::NodeBuilder::behind_gateway).
    /// A node built behind that gateway already dials it again at once.
    ///
    /// [`Gateway::pairing`]: crate::Gateway::pairing
    pub async fn pair_gateway(&self, ticket: &Ticket) -> Result<PeerAddr, PairError> {
        let gateway = self.redeem(ticket, GATEWAY_APP).await?;
        if let Some(uplink) = &self.uplink {
            uplink.retry();
        }
        Ok(gateway)
    }

    async fn redeem(&self, ticket: &Ticket, door: &str) -> Result<PeerAddr, PairError> {
        let client = self
            .connect(ticket.addr.clone(), door)
            .await?
            .with_credential(ticket.token.as_bytes().to_vec());
        client.call(PAIR_METHOD, Value::Null).await?;
        Ok(ticket.addr.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::{NodeKey, PeerId};

    // Written by the first version of pairing, which spoke iroh's types
    // directly. Peer files and tickets already in use must keep reading the
    // same way, whatever carries them.
    const PEER: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";
    const TICKET: &str = "KURI-LINK:UVQXMALCNFSFQIHKJJWGHYU4KIFL55KQPMJS5RPZSVDXNLV6XZ5ZEQQ65JURIRWSFRRGS4ECOAYTSMROGAXDELRUGA5DIMJSGM2HIWZSGAYDCOTEMI4DUORUGBOTUNBRGIZTIZLSMVWGC6MBPANGQ5DUOBZTULZPOJSWYYLZFZSXQYLNOBWGKLTPOJTS6ZLUN5VWK3SQCEIRCEIRCEIRCEIRCEIRCEIRCE";
    const BARE: &str = "KURI-LINK:UNQXMALCNFSFQIHKJJWGHYU4KIFL55KQPMJS5RPZSVDXNLV6XZ5ZEQQ65JURIRWSFRSXI33LMVXFAIRCEIRCEIRCEIRCEIRCEIRCEIQ";

    #[test]
    fn keys_and_tickets_read_and_write_as_they_always_have() {
        let id = NodeKey::from_bytes(&[7; 32]).public();
        assert_eq!(id.to_string(), PEER);
        assert_eq!(PEER.parse::<PeerId>().unwrap(), id);
        assert_eq!(PEER.to_uppercase().parse::<PeerId>().unwrap(), id);

        let addr = PeerAddr::new(id)
            .with_addr("192.0.2.40:41234".parse().unwrap())
            .with_addr("[2001:db8::40]:41234".parse().unwrap())
            .with_relay("https://relay.example.org".parse().unwrap());
        let ticket = Ticket {
            addr,
            token: PairToken([0x11; 16]),
        };
        assert_eq!(ticket.to_string(), TICKET);
        assert_eq!(TICKET.parse::<Ticket>().unwrap(), ticket);

        let bare = Ticket {
            addr: PeerAddr::new(id),
            token: PairToken([0x22; 16]),
        };
        assert_eq!(bare.to_string(), BARE);
        assert_eq!(BARE.parse::<Ticket>().unwrap(), bare);
    }
}
