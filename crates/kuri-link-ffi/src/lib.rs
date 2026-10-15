//! The surface a Dart front end binds to.
//!
//! Every type here is plain — strings, byte vectors, simple structs — so that
//! `flutter_rust_bridge_codegen` can generate the Dart side without hand-written
//! glue. Arguments and results cross as CBOR bytes: the Dart side encodes them
//! (the `cbor` package), which keeps one path for both compiled front ends and
//! front ends that only hold untyped maps (a UI described as data).
//!
//! Errors cross as strings. A typed error enum is worth adding once the Dart
//! side exists to consume it.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

use kuri_link::{EndpointAddr, EndpointId, Node, RelayUrl, Relays, Value};

/// How to reach a peer: its key, plus any addresses known out of band.
#[derive(Debug, Clone)]
pub struct PeerAddr {
    /// The peer's public key, as `EndpointId`'s string form.
    pub id: String,
    /// Direct socket addresses, e.g. `"192.168.86.40:41234"`.
    pub addrs: Vec<String>,
    /// A relay the peer is reachable through, if any.
    pub relay: Option<String>,
}

impl PeerAddr {
    fn parse(&self) -> Result<EndpointAddr, String> {
        let id = EndpointId::from_str(&self.id).map_err(|e| format!("peer id: {e}"))?;
        let mut addr = EndpointAddr::new(id);
        for a in &self.addrs {
            let socket = SocketAddr::from_str(a).map_err(|e| format!("address {a}: {e}"))?;
            addr = addr.with_ip_addr(socket);
        }
        if let Some(relay) = &self.relay {
            let url = RelayUrl::from_str(relay).map_err(|e| format!("relay {relay}: {e}"))?;
            addr = addr.with_relay_url(url);
        }
        Ok(addr)
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

/// This device's node. One per app process is enough.
pub struct LinkNode {
    inner: Node,
}

impl LinkNode {
    /// Starts a node whose identity lives in the file at `key_path`.
    /// `relays` empty means direct paths only.
    pub async fn start(key_path: String, relays: Vec<String>) -> Result<LinkNode, String> {
        let key = kuri_link::load_or_create_key(&PathBuf::from(&key_path))
            .map_err(|e| format!("key file {key_path}: {e}"))?;
        let relays = if relays.is_empty() {
            Relays::None
        } else {
            Relays::Custom(
                relays
                    .iter()
                    .map(|r| RelayUrl::from_str(r).map_err(|e| format!("relay {r}: {e}")))
                    .collect::<Result<_, _>>()?,
            )
        };
        let inner = Node::builder()
            .secret_key(key)
            .relays(relays)
            .bind()
            .await
            .map_err(|e| e.to_string())?;
        Ok(LinkNode { inner })
    }

    /// This device's public key — what a pairing QR code would carry.
    pub fn id(&self) -> String {
        self.inner.id().to_string()
    }

    /// Opens `app` on whichever back end the configuration names.
    pub async fn open_app(
        &self,
        app: String,
        backend: Backend,
        credential: Option<Vec<u8>>,
    ) -> Result<LinkClient, String> {
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
    ) -> Result<LinkClient, String> {
        let addr = peer.parse()?;
        let mut client = self
            .inner
            .connect(addr, &app)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(credential) = credential {
            client = client.with_credential(credential);
        }
        Ok(LinkClient { inner: client })
    }
}

/// Opens the in-process back end registered as `app`.
pub fn open_local(app: String) -> Result<LinkClient, String> {
    kuri_link::Client::local(&app)
        .map(|inner| LinkClient { inner })
        .ok_or_else(|| format!("no local back end is registered as {app}"))
}

/// A connection to one application's back end, local or remote.
pub struct LinkClient {
    inner: kuri_link::Client,
}

impl LinkClient {
    /// `true` when the back end runs in this process.
    pub fn is_local(&self) -> bool {
        self.inner.peer().is_none()
    }

    /// Calls `method` with CBOR-encoded `args`; returns the CBOR-encoded result.
    pub async fn call(&self, method: String, args_cbor: Vec<u8>) -> Result<Vec<u8>, String> {
        let args: Value = if args_cbor.is_empty() {
            Value::Null
        } else {
            ciborium::from_reader(args_cbor.as_slice()).map_err(|e| format!("args: {e}"))?
        };
        let out = self
            .inner
            .call(&method, args)
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        ciborium::into_writer(&out, &mut buf).map_err(|e| format!("result: {e}"))?;
        Ok(buf)
    }
}
