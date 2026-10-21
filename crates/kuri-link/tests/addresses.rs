//! A back end's address as its front ends know it: told by the back end,
//! dialed from then on, and given up on quickly when it cannot be reached
//! from here. Over real iroh endpoints, with relays, port mapping and mDNS
//! off, so these tests need no router and no third party.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use kuri_link::{
    Allowlist, AuthRequest, Authorizer, BoxFuture, Caller, Denied, Methods, Node, NodeKey,
    PeerAddr, PeerId, Relays, Route, Value, ADDR_METHOD,
};

const APP: &str = "echo";

fn echo() -> Methods {
    Methods::new().unary("echo", |_caller, args| async move { Ok(args) })
}

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

async fn client_with(key: NodeKey) -> Node {
    Node::builder()
        .secret_key(key)
        .relays(Relays::None)
        .bind_addr(loopback())
        .bind()
        .await
        .expect("client binds")
}

/// A free port, found by binding and letting go.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The back end goes away and comes back on the same key and port, while the
/// client knows it only at its loopback address, which the back end does not
/// list. Once the client has made a request it learns the back end's own
/// addresses, and dials one of those after the restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_learns_its_back_ends_address_and_dials_that() {
    let key = NodeKey::generate();
    let paired = key.public();
    let server_key = NodeKey::generate();
    let port = free_port();
    let serve = || {
        Node::builder()
            .secret_key(server_key.clone())
            .relays(Relays::None)
            .port(port)
            .port_mapping(false)
            .local_discovery(false)
            .authorizer(Allowlist::new([paired]))
            .serve(APP, echo())
            .bind()
    };
    let server = serve().await.unwrap();
    let listed = server.addr();
    let Some(&elsewhere) = listed.addrs.iter().find(|a| a.is_ipv4()) else {
        // A host with no interface but loopback: nothing to learn here.
        server.shutdown().await;
        return;
    };
    let given = PeerAddr::new(server.id()).with_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    assert!(!listed
        .addrs
        .contains(&given.addrs.first().copied().unwrap()));

    // On every interface: a socket on loopback reaches nothing else.
    let client_node = Node::builder()
        .secret_key(key)
        .relays(Relays::None)
        .port_mapping(false)
        .local_discovery(false)
        .bind()
        .await
        .unwrap();
    let client = client_node.connect(given.clone(), APP).await.unwrap();
    assert_eq!(client.peer_addr(), Some(given));
    let mut updates = client.peer_addr_updates().unwrap();
    let hi = Value::Text("hi".into());
    assert_eq!(client.call("echo", hi.clone()).await.unwrap(), hi);
    tokio::time::timeout(Duration::from_secs(5), updates.changed())
        .await
        .expect("the back end tells its address")
        .unwrap();
    let learned = client.peer_addr().unwrap();
    assert_eq!(learned.id, server.id());
    assert!(learned.addrs.contains(&elsewhere), "learned {learned:?}");

    server.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while client.path().is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a lost connection has no path");
    let server = serve().await.unwrap();
    assert_eq!(client.call("echo", hi.clone()).await.unwrap(), hi);
    let Some(Route::Direct(used)) = client.path().map(|p| p.route) else {
        panic!("no direct path: {:?}", client.path());
    };
    assert!(!used.ip().is_loopback(), "dialed {used}");

    client_node.shutdown().await;
    server.shutdown().await;
}

/// Admits `peer` to everything but address updates.
struct NoUpdates(PeerId);

impl Authorizer for NoUpdates {
    fn authorize<'a>(&'a self, req: AuthRequest<'a>) -> BoxFuture<'a, Result<Caller, Denied>> {
        let admitted = req.peer == self.0 && req.method != ADDR_METHOD;
        Box::pin(async move {
            if admitted {
                Ok(Caller {
                    origin: kuri_link::Origin::Peer(req.peer),
                    claims: Value::Null,
                })
            } else {
                Err(Denied("no".into()))
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_back_end_that_refuses_updates_still_serves() {
    let key = NodeKey::generate();
    let server = Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .authorizer(NoUpdates(key.public()))
        .serve(APP, echo())
        .bind()
        .await
        .unwrap();
    let given = server.bound_addr();
    let client_node = client_with(key).await;
    let client = client_node.connect(given.clone(), APP).await.unwrap();
    let mut updates = client.peer_addr_updates().unwrap();
    for _ in 0..3 {
        let hi = Value::Text("hi".into());
        assert_eq!(client.call("echo", hi.clone()).await.unwrap(), hi);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), updates.changed())
            .await
            .is_err(),
        "no update came"
    );
    assert_eq!(client.peer_addr(), Some(given));

    client_node.shutdown().await;
    server.shutdown().await;
}

/// Every address a private one, and no relay: from anywhere but that network
/// nothing will ever answer, so waiting iroh's 30 seconds helps nobody.
#[tokio::test(flavor = "multi_thread")]
async fn a_back_end_reachable_only_on_its_own_network_is_given_up_on_soon() {
    let client_node = client_with(NodeKey::generate()).await;
    let elsewhere = PeerAddr::new(NodeKey::generate().public())
        .with_addr("192.168.250.77:47555".parse().unwrap());
    let start = Instant::now();
    let e = client_node.connect(elsewhere, APP).await.unwrap_err();
    let took = start.elapsed();
    assert!(took < Duration::from_secs(8), "took {took:?}");
    assert!(
        e.to_string()
            .contains("only a device on that network can reach it"),
        "{e}"
    );
    client_node.shutdown().await;
}
