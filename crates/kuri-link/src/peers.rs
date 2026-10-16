//! The peers a node admits: shared, mutable, and optionally kept in a file.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use ciborium::Value;
use iroh::EndpointId;

use crate::hooks::{AuthRequest, Authorizer, Caller, Denied, Origin};
use crate::BoxFuture;

/// The keys a node admits. Cheap to clone: clones share one set, so the code
/// that adds peers and the authorizer that checks them always agree.
///
/// Opened on a file, every change is written through before the call returns:
/// one key per line, the file replaced atomically and kept mode 0600, so a
/// restart keeps every pairing. If the write fails the change is undone and
/// the error returned, so the set in memory never admits more than the file.
/// The write is synchronous (a small file and an fsync); async code that
/// changes the set often should do it on a blocking thread.
///
/// As an [`Authorizer`] it admits exactly its members, re-checked on every
/// request: a peer removed while connected is refused from its next request.
#[derive(Clone, Default)]
pub struct PeerSet {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Default)]
struct Inner {
    peers: BTreeSet<EndpointId>,
    file: Option<PathBuf>,
}

impl PeerSet {
    /// An empty set kept only in memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// The set kept in the file at `path`, read now if the file exists.
    ///
    /// Blank lines and lines starting with `#` are ignored, so the file can be
    /// annotated by hand; annotations do not survive the next change.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let peers = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
            Err(e) => return Err(e),
        };
        Ok(PeerSet {
            inner: Arc::new(RwLock::new(Inner {
                peers,
                file: Some(path),
            })),
        })
    }

    pub fn contains(&self, peer: &EndpointId) -> bool {
        self.read().peers.contains(peer)
    }

    /// The peers, in a stable order.
    pub fn peers(&self) -> Vec<EndpointId> {
        self.read().peers.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.read().peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.read().peers.is_empty()
    }

    /// Admits `peer`. Returns `false` if it was already admitted.
    pub fn insert(&self, peer: EndpointId) -> io::Result<bool> {
        let mut inner = self.write();
        if !inner.peers.insert(peer) {
            return Ok(false);
        }
        if let Err(e) = inner.persist() {
            inner.peers.remove(&peer);
            return Err(e);
        }
        Ok(true)
    }

    /// Stops admitting `peer`. Returns `false` if it was not admitted.
    pub fn remove(&self, peer: &EndpointId) -> io::Result<bool> {
        let mut inner = self.write();
        if !inner.peers.remove(peer) {
            return Ok(false);
        }
        if let Err(e) = inner.persist() {
            inner.peers.insert(*peer);
            return Err(e);
        }
        Ok(true)
    }

    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl fmt::Debug for PeerSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.read();
        f.debug_struct("PeerSet")
            .field("peers", &inner.peers.len())
            .field("file", &inner.file)
            .finish()
    }
}

impl Authorizer for PeerSet {
    fn authorize<'a>(&'a self, req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>> {
        let admitted = self.contains(&req.peer);
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

impl Inner {
    fn persist(&self) -> io::Result<()> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        let mut text = String::new();
        for peer in &self.peers {
            text.push_str(&peer.to_string());
            text.push('\n');
        }
        write_atomically(path, text.as_bytes())
    }
}

fn parse(text: &str) -> io::Result<BTreeSet<EndpointId>> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            EndpointId::from_str(line).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("peer {line}: {e}"))
            })
        })
        .collect()
}

/// Writes `bytes` beside `path`, then renames over it, so a reader sees the
/// old file or the new one and never half of either.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp)?;
    // `mode` applies only when the file is created; a leftover from a crash
    // keeps whatever it had.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    // The rename lives in the directory; until that is synced too, a power
    // cut can bring back the old file and drop the change.
    #[cfg(unix)]
    {
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}
