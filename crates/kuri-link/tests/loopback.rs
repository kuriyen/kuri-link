//! End-to-end over real iroh endpoints on the loopback interface, with relays
//! off — so these tests need no network and no third party.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use kuri_link::{
    Allowlist, BoxFuture, CallError, Client, FaultCode, Methods, Node, NodeKey, PeerAddr, Relays,
    Value, Waker,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const APP: &str = "echo";

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

fn methods() -> Methods {
    Methods::new()
        .unary("echo", |_caller, args| async move { Ok(args) })
        .typed(
            "add",
            |_caller, (a, b): (i64, i64)| async move { Ok(a + b) },
        )
        .typed("whoami", |caller, (): ()| async move {
            Ok(caller
                .peer()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "in-process".into()))
        })
        .duplex("upper", |_caller, _args, mut io| async move {
            let fail = |e: std::io::Error| kuri_link::AppError::Failed(e.to_string());
            let mut data = Vec::new();
            io.recv.read_to_end(&mut data).await.map_err(fail)?;
            io.send
                .write_all(&data.to_ascii_uppercase())
                .await
                .map_err(fail)?;
            io.send.shutdown().await.map_err(fail)?;
            Ok(())
        })
}

async fn server_for(paired: &[NodeKey]) -> Node {
    Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .authorizer(Allowlist::new(paired.iter().map(|k| k.public())))
        .serve(APP, methods())
        .bind()
        .await
        .expect("server binds")
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

fn fault_code(e: CallError) -> FaultCode {
    match e {
        CallError::Fault(fault) => fault.code,
        other => panic!("expected a fault, got {other}"),
    }
}

/// The same assertions, run against a back end over the network and one in
/// this process: a front end must not be able to tell them apart.
async fn exercise(client: &Client, expected_caller: &str) {
    let hi = Value::Text("hi".into());
    assert_eq!(client.call("echo", hi.clone()).await.unwrap(), hi);

    let sum: i64 = client.call_typed("add", &(2i64, 3i64)).await.unwrap();
    assert_eq!(sum, 5);

    // Over the network the server sees the key the handshake proved, not
    // anything the client said; in-process there is no key at all.
    let seen: String = client.call_typed("whoami", &()).await.unwrap();
    assert_eq!(seen, expected_caller);

    let mut io = client
        .open("upper", Value::Null)
        .await
        .expect("open accepted");
    io.send.write_all(b"kuri-link").await.unwrap();
    io.send.shutdown().await.unwrap();
    let mut out = Vec::new();
    io.recv.read_to_end(&mut out).await.unwrap();
    assert_eq!(out, b"KURI-LINK");

    let e = client.call("nope", Value::Null).await.unwrap_err();
    assert_eq!(fault_code(e), FaultCode::NoSuchMethod);
    let e = client.open("echo", Value::Null).await.unwrap_err();
    assert_eq!(
        fault_code(e),
        FaultCode::NoSuchMethod,
        "echo is not an open method"
    );
    let e = client
        .call("add", Value::Text("not a pair".into()))
        .await
        .unwrap_err();
    assert_eq!(fault_code(e), FaultCode::BadRequest);

    // Requests run concurrently, each on its own stream.
    let calls = (0..16i64).map(|i| {
        let client = client.clone();
        async move { client.call_typed::<_, i64>("add", &(i, i)).await.unwrap() }
    });
    let sums: Vec<i64> = join_all(calls).await;
    assert_eq!(sums, (0..16).map(|i| 2 * i).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_peer_and_an_in_process_back_end_behave_the_same() {
    let key = NodeKey::generate();
    let server = server_for(std::slice::from_ref(&key)).await;
    let client_node = client_with(key.clone()).await;
    let remote = client_node
        .connect(server.bound_addr(), APP)
        .await
        .expect("connects");
    assert_eq!(remote.peer(), Some(server.id()));
    exercise(&remote, &key.public().to_string()).await;

    let local = Client::in_process(methods());
    assert_eq!(local.peer(), None);
    exercise(&local, "in-process").await;

    client_node.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registered_local_back_end_is_found_by_name() {
    kuri_link::register_local("registered-app", methods());
    let client = Client::local("registered-app").expect("registered");
    exercise(&client, "in-process").await;
    assert!(Client::local("never-registered").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unpaired_key_is_denied_on_every_request() {
    let paired = NodeKey::generate();
    let stranger = NodeKey::generate();
    let server = server_for(&[paired]).await;
    let client_node = client_with(stranger).await;
    let client = client_node.connect(server.bound_addr(), APP).await.unwrap();

    let e = client.call("echo", Value::Null).await.unwrap_err();
    assert_eq!(fault_code(e), FaultCode::Denied);
    let e = client.open("upper", Value::Null).await.unwrap_err();
    assert_eq!(fault_code(e), FaultCode::Denied);

    client_node.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_authorizer_serves_nobody() {
    let key = NodeKey::generate();
    let server = Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .serve(APP, methods())
        .bind()
        .await
        .unwrap();
    let client_node = client_with(key).await;
    let client = client_node.connect(server.bound_addr(), APP).await.unwrap();

    let e = client.call("echo", Value::Null).await.unwrap_err();
    assert_eq!(fault_code(e), FaultCode::Denied);

    client_node.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_the_peer_does_not_serve_fails_at_the_handshake() {
    let key = NodeKey::generate();
    let server = server_for(std::slice::from_ref(&key)).await;
    let client_node = client_with(key).await;

    assert!(client_node
        .connect(server.bound_addr(), "not-served")
        .await
        .is_err());

    client_node.shutdown().await;
    server.shutdown().await;
}

#[derive(Default, Clone)]
struct CountingWaker(Arc<AtomicUsize>);

impl Waker for CountingWaker {
    fn wake<'a>(&'a self, _peer: &'a PeerAddr, _app: &'a str) -> BoxFuture<'a, bool> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { false })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_waker_gets_one_chance_when_connecting_fails() {
    let waker = CountingWaker::default();
    let client_node = Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .waker(waker.clone())
        .bind()
        .await
        .unwrap();

    // A peer that serves the app on another key: the handshake fails.
    let other = server_for(&[]).await;
    let wrong = PeerAddr::new(NodeKey::generate().public())
        .with_addr(*other.bound_addr().addrs.iter().next().unwrap());
    assert!(client_node.connect(wrong, APP).await.is_err());
    assert_eq!(waker.0.load(Ordering::SeqCst), 1);

    client_node.shutdown().await;
    other.shutdown().await;
}

/// `futures::future::join_all` without the dependency, on spawned tasks.
async fn join_all<F: std::future::Future + Send + 'static>(
    futs: impl Iterator<Item = F>,
) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("task"));
    }
    out
}
