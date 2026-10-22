//! Kuri Link's own names for keys and addresses.
//!
//! iroh carries them underneath, but nothing public here is an iroh type, so
//! moving to a new iroh major version, or to another transport, does not
//! change this crate's API. The text forms are fixed here too: a peer file or
//! a ticket written today reads back after such a move.

use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use data_encoding::{HEXLOWER, HEXLOWER_PERMISSIVE};
use serde::{Deserialize, Serialize};

/// A string or byte form that does not hold a key or an address.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(String);

/// A node's public key: who it is, and what peers dial.
///
/// The text form is the key's 32 bytes as 64 lowercase hex digits, which is
/// what a peer file holds. Reading accepts either case.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(iroh::PublicKey);

impl PeerId {
    /// The key from its bytes. Refuses bytes that are not an Ed25519 public
    /// key.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<PeerId, ParseError> {
        iroh::PublicKey::from_bytes(bytes)
            .map(PeerId)
            .map_err(|_| ParseError("not an Ed25519 public key".into()))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// The first 10 hex digits, enough to tell keys apart in a log.
    pub fn fmt_short(&self) -> String {
        HEXLOWER.encode(&self.as_bytes()[..5])
    }

    pub(crate) fn from_iroh(key: iroh::PublicKey) -> PeerId {
        PeerId(key)
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&HEXLOWER.encode(self.as_bytes()))
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({self})")
    }
}

impl FromStr for PeerId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = HEXLOWER_PERMISSIVE
            .decode(s.as_bytes())
            .map_err(|e| ParseError(format!("not hex: {e}")))?;
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| ParseError(format!("{} bytes, not 32", bytes.len())))?;
        PeerId::from_bytes(&bytes)
    }
}

/// A node's secret key. Whoever holds it is that node; keep it like a
/// password. `Debug` shows only the public half.
#[derive(Clone)]
pub struct NodeKey(iroh::SecretKey);

impl NodeKey {
    /// A fresh random key.
    pub fn generate() -> NodeKey {
        NodeKey(iroh::SecretKey::generate())
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> NodeKey {
        NodeKey(iroh::SecretKey::from_bytes(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// The public half: the [`PeerId`] this key makes a node.
    pub fn public(&self) -> PeerId {
        PeerId(self.0.public())
    }

    pub(crate) fn into_iroh(self) -> iroh::SecretKey {
        self.0
    }
}

impl fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeKey(public: {})", self.public().fmt_short())
    }
}

/// The URL of a relay a peer can be reached through.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelayUrl(iroh::RelayUrl);

impl RelayUrl {
    pub(crate) fn from_iroh(url: iroh::RelayUrl) -> RelayUrl {
        RelayUrl(url)
    }

    pub(crate) fn to_iroh(&self) -> iroh::RelayUrl {
        self.0.clone()
    }
}

impl fmt::Display for RelayUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Debug for RelayUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelayUrl({self})")
    }
}

impl FromStr for RelayUrl {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        iroh::RelayUrl::from_str(s)
            .map(RelayUrl)
            .map_err(|e| ParseError(format!("relay URL {s}: {e}")))
    }
}

/// How to reach a peer: its key, plus any direct addresses and relays known
/// for it. The key alone is enough on paths that can look it up; on a LAN with
/// relays off, give a socket address too.
///
/// The text form, for keeping an address in a file, is the key, then each
/// direct address and each relay, separated by spaces:
/// `ea4a…d22c 192.0.2.40:41234 https://relay.example.org/`. Reading it back
/// accepts any whitespace between them, so a file may hold one to a line.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerAddr {
    pub id: PeerId,
    /// Direct UDP sockets, e.g. `192.0.2.40:41234`.
    pub addrs: BTreeSet<SocketAddr>,
    /// Relays the peer is reachable through.
    pub relays: BTreeSet<RelayUrl>,
}

impl PeerAddr {
    /// Just the key, with no address known yet.
    pub fn new(id: PeerId) -> PeerAddr {
        PeerAddr {
            id,
            addrs: BTreeSet::new(),
            relays: BTreeSet::new(),
        }
    }

    pub fn with_addr(mut self, addr: SocketAddr) -> PeerAddr {
        self.addrs.insert(addr);
        self
    }

    pub fn with_relay(mut self, relay: RelayUrl) -> PeerAddr {
        self.relays.insert(relay);
        self
    }

    /// From the parts a ticket or an address update carries: the key's
    /// bytes, and the addresses and relays as text.
    pub(crate) fn from_parts(
        id: &[u8],
        ips: &[String],
        relays: &[String],
    ) -> Result<PeerAddr, ParseError> {
        let id: [u8; 32] = id
            .try_into()
            .map_err(|_| ParseError("the key is not 32 bytes".into()))?;
        let id = PeerId::from_bytes(&id).map_err(|e| ParseError(format!("key: {e}")))?;
        let mut addr = PeerAddr::new(id);
        for ip in ips {
            let socket: SocketAddr = ip
                .parse()
                .map_err(|e| ParseError(format!("address {ip}: {e}")))?;
            addr = addr.with_addr(socket);
        }
        for relay in relays {
            addr = addr.with_relay(RelayUrl::from_str(relay)?);
        }
        Ok(addr)
    }

    pub(crate) fn from_iroh(addr: &iroh::EndpointAddr) -> PeerAddr {
        PeerAddr {
            id: PeerId(addr.id),
            addrs: addr.ip_addrs().copied().collect(),
            relays: addr.relay_urls().cloned().map(RelayUrl).collect(),
        }
    }

    pub(crate) fn to_iroh(&self) -> iroh::EndpointAddr {
        let mut addr = iroh::EndpointAddr::new(self.id.0);
        for socket in &self.addrs {
            addr = addr.with_ip_addr(*socket);
        }
        for relay in &self.relays {
            addr = addr.with_relay_url(relay.0.clone());
        }
        addr
    }
}

/// A [`PeerAddr`] as a back end sends its own to its front ends. Named keys,
/// as everywhere on the wire, so a later version can add fields that this one
/// ignores.
#[derive(Serialize, Deserialize)]
pub(crate) struct AddrWire {
    id: serde_bytes::ByteBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ip: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    relay: Vec<String>,
}

impl AddrWire {
    pub(crate) fn new(addr: &PeerAddr) -> AddrWire {
        AddrWire {
            id: serde_bytes::ByteBuf::from(addr.id.as_bytes().to_vec()),
            ip: addr.addrs.iter().map(|a| a.to_string()).collect(),
            relay: addr.relays.iter().map(|u| u.to_string()).collect(),
        }
    }

    pub(crate) fn to_addr(&self) -> Result<PeerAddr, ParseError> {
        PeerAddr::from_parts(&self.id, &self.ip, &self.relay)
    }
}

impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)?;
        for socket in &self.addrs {
            write!(f, " {socket}")?;
        }
        for relay in &self.relays {
            write!(f, " {relay}")?;
        }
        Ok(())
    }
}

impl FromStr for PeerAddr {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut words = s.split_whitespace();
        let id = words
            .next()
            .ok_or_else(|| ParseError("an address starts with the peer's key".into()))?;
        let mut addr = PeerAddr::new(id.parse()?);
        for word in words {
            addr = match word.parse::<SocketAddr>() {
                Ok(socket) => addr.with_addr(socket),
                Err(_) => addr.with_relay(word.parse()?),
            };
        }
        Ok(addr)
    }
}

impl From<PeerId> for PeerAddr {
    fn from(id: PeerId) -> PeerAddr {
        PeerAddr::new(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_reads_back_from_its_text() {
        let id = NodeKey::from_bytes(&[7; 32]).public();
        let addr = PeerAddr::new(id)
            .with_addr("192.0.2.40:41234".parse().unwrap())
            .with_addr("[2001:db8::40]:41234".parse().unwrap())
            .with_relay("https://relay.example.org".parse().unwrap());
        let text = addr.to_string();
        assert_eq!(
            text,
            format!("{id} 192.0.2.40:41234 [2001:db8::40]:41234 https://relay.example.org/")
        );
        assert_eq!(text.parse::<PeerAddr>().unwrap(), addr);
        assert_eq!(
            text.replace(' ', "\n").parse::<PeerAddr>().unwrap(),
            addr,
            "one to a line"
        );
        assert_eq!(
            id.to_string().parse::<PeerAddr>().unwrap(),
            PeerAddr::new(id)
        );
        assert!("".parse::<PeerAddr>().is_err());
        assert!(format!("{id} not-an-address").parse::<PeerAddr>().is_err());
    }
}
