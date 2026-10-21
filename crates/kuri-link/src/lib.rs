//! Kuri Link — private RPC between a front end and its back end over iroh.
//!
//! Peers dial each other by public key, not by address. iroh finds a path —
//! same LAN, a direct hole-punched UDP path through NAT, or a relay that only
//! ever sees ciphertext — and authenticates both ends by their keys during the
//! QUIC/TLS handshake. Kuri Link adds a small request layer on top:
//!
//! - **One request per QUIC bidirectional stream.** A slow request never
//!   blocks another (no head-of-line blocking between calls).
//! - **Two operations.** `call` sends arguments and gets one reply. `open`
//!   sends arguments, gets an accept/refuse reply, then hands both ends a raw
//!   byte stream — for media, files, or anything that is not a request.
//! - **CBOR payloads with named keys**, so a newer peer can add optional
//!   fields without breaking an older one, and a front end that holds only
//!   untyped data (a map from a UI description) can still make calls.
//! - **The back end can be local or remote, chosen at run time.** A
//!   [`Client`] for a back end linked into the same process behaves exactly
//!   like one for a back end across the network — same calls, same faults —
//!   so an app switches between the two by configuration.
//! - **Hooks, not policy.** Who may call ([`Authorizer`]), which relays to
//!   trust ([`Relays`]), and what to do when the back end is asleep
//!   ([`Waker`]) are supplied by the application. Nothing defaults to a third
//!   party: relays are off unless chosen, and the default authorizer refuses
//!   everyone.
//! - **Its own names for keys and addresses.** [`PeerId`], [`PeerAddr`] and
//!   [`NodeKey`] are Kuri Link's types; iroh stays underneath, so a change of
//!   transport is not a change of API.
//! - **Pairing by QR code.** [`Pairing`] issues one-time [`Ticket`]s: the back
//!   end's address and a short-lived token, as text a QR code carries. A front
//!   end redeems one with [`Node::pair`], and the back end adds the key the
//!   handshake proved to a [`PeerSet`], which can be kept in a file.
//!
//! See `docs/design.md` for the wire format and the reasoning.

mod addr;
mod hooks;
mod local;
mod node;
mod pairing;
mod peers;
mod portmap;
mod service;
mod wire;

use std::future::Future;
use std::pin::Pin;

pub use addr::{NodeKey, ParseError, PeerAddr, PeerId, RelayUrl};
pub use ciborium::Value;

pub use hooks::{
    load_or_create_key, Allowlist, AuthRequest, Authorizer, Caller, Denied, DenyAll, Origin, Waker,
};
pub use local::register_local;
pub use node::{CallError, Client, Error, Node, NodeBuilder, PathInfo, Reach, Relays, Route};
pub use pairing::{PairError, PairToken, Pairing, Ticket, TicketError, PAIR_APP, PAIR_METHOD};
pub use peers::PeerSet;
pub use service::{from_value, to_value, AppError, Duplex, Methods, OpenHandler, Service};
pub use wire::{Fault, FaultCode, WireError, DEFAULT_MAX_FRAME, WIRE_VERSION};

/// A boxed, sendable future — the return type of the hook and service traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The ALPN an application is served under: `kuri-link/<wire version>/<app>`.
///
/// One ALPN per application and wire version, so a peer that does not serve
/// the application — or speaks another wire version — refuses the connection
/// during the handshake, before any request is read.
pub fn alpn(app: &str) -> Vec<u8> {
    format!("kuri-link/{WIRE_VERSION}/{app}").into_bytes()
}
