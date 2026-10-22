//! The surface a Dart front end binds to, driven from Rust: pair with a back
//! end by the text of its QR code, then call it.

use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use kuri_link::{Methods, Node, NodeKey, Pairing, PeerSet, RelayUrl, Relays};
use kuri_link_ffi::{Backend, LinkNode};

#[tokio::test(flavor = "multi_thread")]
async fn a_front_end_pairs_by_ticket_text_then_calls() {
    let pairing = Pairing::new(PeerSet::new());
    let echo = Methods::new().unary("echo", |_caller, args| async move { Ok(args) });
    let server = Node::builder()
        .relays(Relays::None)
        .bind_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .pairing(pairing.clone())
        .serve("echo", echo)
        .bind()
        .await
        .unwrap();
    // Every relay the back end names comes back, not just the first.
    let relays = ["https://relay.example.org", "https://relay.example.net"]
        .map(|r| RelayUrl::from_str(r).unwrap());
    let addr = relays
        .iter()
        .fold(server.bound_addr(), |addr, r| addr.with_relay(r.clone()));
    let ticket = pairing.issue(addr, Duration::from_secs(60));

    let name = format!("kuri-link-{}.key", NodeKey::generate().public().fmt_short());
    let key = std::env::temp_dir().join(name);
    let node = LinkNode::start(key.display().to_string(), Vec::new(), None)
        .await
        .unwrap();
    assert!(
        node.pair("KURI-LINK:".into()).await.is_err(),
        "not a ticket"
    );
    let peer = node.pair(ticket.to_string()).await.expect("pairs");
    assert_eq!(peer.id, server.id().to_string());
    let mut got = peer.relays.clone();
    got.sort();
    let mut want: Vec<String> = relays.iter().map(|r| r.to_string()).collect();
    want.sort();
    assert_eq!(got, want);

    let backend = Backend::Remote { peer };
    let client = node.open_app("echo".into(), backend, None).await.unwrap();
    assert!(!client.is_local());
    let mut hi = Vec::new();
    ciborium::into_writer("hi", &mut hi).unwrap();
    assert_eq!(client.call("echo".into(), hi.clone()).await.unwrap(), hi);

    let _ = std::fs::remove_file(&key);
    server.shutdown().await;
}
