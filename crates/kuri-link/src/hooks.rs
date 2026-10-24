//! The application's hooks: identity, authorization, and waking.

use std::collections::HashSet;
use std::io;
use std::path::Path;

use crate::addr::{NodeKey, PeerAddr, PeerId};
use ciborium::Value;

use crate::BoxFuture;

/// Loads a node's secret key from `path`, creating one if the file does not
/// exist. The file holds the raw 32 key bytes; on Unix it is created mode 0600.
///
/// A node's key *is* its address: peers dial the public half. Keep the file
/// for as long as the node should stay reachable at the same identity.
pub fn load_or_create_key(path: &Path) -> io::Result<NodeKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "key file is not 32 bytes")
            })?;
            Ok(NodeKey::from_bytes(&bytes))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = NodeKey::generate();
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            use std::io::Write;
            opts.open(path)?.write_all(&key.to_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e),
    }
}

/// What the authorizer is asked about one request. More fields may come, so
/// only Kuri Link makes one.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct AuthRequest<'a> {
    /// The caller's key, proven during the QUIC/TLS handshake. Not a claim:
    /// the connection could not exist unless the peer holds the private half.
    /// On a back end behind a gateway, it is the key the gateway proved and
    /// vouches for.
    pub peer: PeerId,
    /// The application the connection was opened for.
    pub app: &'a str,
    /// The method being called or opened.
    pub method: &'a str,
    /// Whatever credential the caller attached, untouched by Kuri Link.
    pub credential: Option<&'a [u8]>,
}

/// Where a request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The back end is linked into the same process as the caller. No network,
    /// no key: the trust boundary is the process itself, so no authorizer runs.
    InProcess,
    /// A peer, identified by the key it proved during the handshake, or, on
    /// a back end behind a gateway, by the key the gateway vouches for.
    Peer(PeerId),
}

/// Who is calling, as the authorizer vouches for them. Handed to the method.
#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub origin: Origin,
    /// Anything the authorizer wants the method to know — a role, a scope.
    pub claims: Value,
}

impl Caller {
    /// The peer's key, or `None` for an in-process call.
    pub fn peer(&self) -> Option<PeerId> {
        match self.origin {
            Origin::Peer(id) => Some(id),
            Origin::InProcess => None,
        }
    }
}

/// A refusal, with a reason the caller is shown.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct Denied(pub String);

/// Decides, per request, whether a caller may proceed.
///
/// Called for every request rather than once per connection, so a credential
/// that expires or is revoked stops working on the next request, not the
/// next reconnect.
pub trait Authorizer: Send + Sync + 'static {
    fn authorize<'a>(&'a self, req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>>;
}

/// Refuses everyone. The default, so a node serves nothing until it is told
/// whom to serve.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAll;

impl Authorizer for DenyAll {
    fn authorize<'a>(&'a self, _req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>> {
        Box::pin(async { Err(Denied("this node authorizes nobody".into())) })
    }
}

/// Admits a fixed set of peer keys, given when the node is built. For a set
/// that grows as devices pair and survives restarts, use
/// [`PeerSet`](crate::PeerSet) with [`Pairing`](crate::Pairing).
#[derive(Debug, Default, Clone)]
pub struct Allowlist {
    peers: HashSet<PeerId>,
}

impl Allowlist {
    pub fn new(peers: impl IntoIterator<Item = PeerId>) -> Self {
        Allowlist {
            peers: peers.into_iter().collect(),
        }
    }

    pub fn contains(&self, peer: &PeerId) -> bool {
        self.peers.contains(peer)
    }
}

impl Authorizer for Allowlist {
    fn authorize<'a>(&'a self, req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>> {
        let admitted = self.peers.contains(&req.peer);
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

/// Called when connecting fails, before one retry: the place to wake a back
/// end that sleeps to save power. Return `true` to retry.
pub trait Waker: Send + Sync + 'static {
    fn wake<'a>(&'a self, peer: &'a PeerAddr, app: &'a str) -> BoxFuture<'a, bool>;
}
