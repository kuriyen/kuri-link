//! A gateway end to end over loopback, relays off: a phone pairs with the
//! gateway once and reaches the back ends behind it, which pair with the
//! gateway themselves and listen for nothing.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use kuri_link::{
    Allowlist, CallError, FaultCode, Gateway, Methods, Node, NodeKey, PairError, Pairing, PeerAddr,
    PeerSet, Registration, RelayUrl, Relays, Value, GATEWAY_APP,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const APP: &str = "echo";
const MINUTE: Duration = Duration::from_secs(60);
/// Long enough for a back end that was refused to dial again after its
/// longest wait.
const PATIENCE: Duration = Duration::from_secs(40);

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

/// `echo`, `whoami`, which names the caller's key, and `upper`, a stream.
fn echo(name: &'static str) -> Methods {
    Methods::new()
        .unary("echo", move |_caller, args| async move {
            Ok(Value::Array(vec![Value::Text(name.into()), args]))
        })
        .typed("whoami", |caller, (): ()| async move {
            Ok(caller.peer().map(|p| p.to_string()))
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

/// A gateway on loopback that pairs phones into `phones`.
async fn gateway_node(gateway: &Gateway, phones: &Pairing, key: NodeKey) -> Node {
    Node::builder()
        .secret_key(key)
        .relays(Relays::None)
        .bind_addr(loopback())
        .pairing(phones.clone())
        .gateway(gateway.clone())
        .bind()
        .await
        .expect("gateway binds")
}

/// A back end behind the gateway at `gateway`, serving `echo(name)` as
/// [`APP`].
async fn back_end(gateway: PeerAddr, key: NodeKey, name: &'static str) -> Node {
    Node::builder()
        .secret_key(key)
        .relays(Relays::None)
        .bind_addr(loopback())
        .serve(APP, echo(name))
        .behind_gateway(gateway)
        .bind()
        .await
        .expect("back end binds")
}

/// A back end that has paired with the gateway and registered there.
async fn paired_back_end(gateway: &Gateway, at: &Node, key: NodeKey, name: &'static str) -> Node {
    let node = back_end(at.bound_addr(), key, name).await;
    let ticket = gateway.pairing().issue(at.bound_addr(), MINUTE);
    node.pair_gateway(&ticket)
        .await
        .expect("the back end pairs");
    settles(&node, |r| *r == Registration::Registered).await;
    node
}

async fn phone() -> Node {
    Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .bind()
        .await
        .expect("phone binds")
}

/// Waits until `node`'s registration is as `wanted` says.
async fn settles(node: &Node, wanted: impl Fn(&Registration) -> bool) {
    let mut registration = node.registration().expect("behind a gateway");
    let settled = tokio::time::timeout(PATIENCE, registration.wait_for(|r| wanted(r))).await;
    match settled {
        Ok(running) => drop(running.expect("the node runs")),
        Err(_) => panic!("still {:?}", *node.registration().unwrap().borrow()),
    }
}

fn fault(e: &CallError) -> Option<FaultCode> {
    match e {
        CallError::Fault(fault) => Some(fault.code),
        _ => None,
    }
}

/// Calls `echo` with `"hi"` and returns which back end answered.
async fn who_echoes(phone: &Node, gateway: &Node) -> Result<String, CallError> {
    let client = phone
        .connect(gateway.bound_addr(), APP)
        .await
        .expect("connects");
    let reply = client.call("echo", Value::Text("hi".into())).await?;
    match reply {
        Value::Array(parts) if parts.get(1) == Some(&Value::Text("hi".into())) => {
            Ok(parts[0].as_text().unwrap().to_owned())
        }
        other => panic!("echo answered {other:?}"),
    }
}

/// A fresh directory under the system's temporary one, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let name = format!("kuri-link-{}", NodeKey::generate().public().fmt_short());
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_paired_with_the_gateway_reaches_the_back_end_behind_it() {
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = gateway_node(&gateway, &phones, NodeKey::generate()).await;
    let camera = paired_back_end(&gateway, &home, NodeKey::generate(), "camera").await;
    assert_eq!(gateway.apps().len(), 1);
    assert!(gateway.apps()[0].online && gateway.apps()[0].owner == camera.id());

    let phone = phone().await;
    let unpaired = phone.connect(home.bound_addr(), APP).await.unwrap();
    let e = unpaired.call("echo", Value::Null).await.unwrap_err();
    assert_eq!(fault(&e), Some(FaultCode::Denied), "unpaired: {e}");

    let at = phone
        .pair(&phones.issue(home.bound_addr(), MINUTE))
        .await
        .expect("the phone pairs with the gateway");
    assert_eq!(at.id, home.id());
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "camera");

    // The back end sees the phone as the caller, not the gateway.
    let client = phone.connect(home.bound_addr(), APP).await.unwrap();
    let caller: Option<String> = client.call_typed("whoami", &()).await.unwrap();
    assert_eq!(caller, Some(phone.id().to_string()));

    // A stream goes through whole, both ways, and ends cleanly.
    let mut io = client.open("upper", Value::Null).await.unwrap();
    io.send.write_all(b"through the gateway").await.unwrap();
    io.send.shutdown().await.unwrap();
    let mut back = Vec::new();
    io.recv.read_to_end(&mut back).await.unwrap();
    assert_eq!(back, b"THROUGH THE GATEWAY");

    // A method the back end lacks is its answer, passed on.
    let e = client.call("nothing", Value::Null).await.unwrap_err();
    assert_eq!(fault(&e), Some(FaultCode::NoSuchMethod), "{e}");

    // The phone learns the gateway's address, not the back end's.
    assert_eq!(client.peer_addr().unwrap().id, home.id());

    // Unpaired at the gateway, the phone is refused from its next request.
    phones.peers().remove(&phone.id()).unwrap();
    let e = client.call("echo", Value::Null).await.unwrap_err();
    assert_eq!(fault(&e), Some(FaultCode::Denied), "{e}");

    phone.shutdown().await;
    camera.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn phones_and_back_ends_each_keep_to_their_own_door() {
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = gateway_node(&gateway, &phones, NodeKey::generate()).await;
    let camera = paired_back_end(&gateway, &home, NodeKey::generate(), "camera").await;

    // A phone's ticket does not open the back ends' door, and stays good.
    let phone = phone().await;
    let ticket = phones.issue(home.bound_addr(), MINUTE);
    let e = phone.pair_gateway(&ticket).await.unwrap_err();
    assert!(
        matches!(&e, PairError::Call(e) if fault(e) == Some(FaultCode::Denied)),
        "{e}"
    );
    phone.pair(&ticket).await.expect("the phone pairs");
    assert!(!gateway.pairing().peers().contains(&phone.id()));

    // Nor does a back end's ticket admit a phone.
    let stranger = self::phone().await;
    let ticket = gateway.pairing().issue(home.bound_addr(), MINUTE);
    let e = stranger.pair(&ticket).await.unwrap_err();
    assert!(
        matches!(&e, PairError::Call(e) if fault(e) == Some(FaultCode::Denied)),
        "{e}"
    );

    // A paired phone cannot register an application…
    let door = phone.connect(home.bound_addr(), GATEWAY_APP).await.unwrap();
    let mut register = door
        .open(
            kuri_link::REGISTER_METHOD,
            Value::Map(vec![(
                Value::Text("apps".into()),
                Value::Array(vec![Value::Text("evil".into())]),
            )]),
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(fault(&register), Some(FaultCode::Denied), "{register}");

    // …and a back end cannot call one.
    let client = camera.connect(home.bound_addr(), APP).await.unwrap();
    register = client.call("echo", Value::Null).await.unwrap_err();
    assert_eq!(fault(&register), Some(FaultCode::Denied), "{register}");

    // The back end listens for nothing: dialing it directly fails.
    assert!(phone.connect(camera.bound_addr(), APP).await.is_err());

    phone.shutdown().await;
    stranger.shutdown().await;
    camera.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_back_end_that_is_gone_is_unavailable_until_it_registers_again() {
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = gateway_node(&gateway, &phones, NodeKey::generate()).await;
    let key = NodeKey::generate();
    let camera = paired_back_end(&gateway, &home, key.clone(), "camera").await;
    let phone = phone().await;
    phone
        .pair(&phones.issue(home.bound_addr(), MINUTE))
        .await
        .unwrap();
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "camera");

    camera.shutdown().await;
    let e = tokio::time::timeout(PATIENCE, async {
        loop {
            match who_echoes(&phone, &home).await {
                Err(e) => break e,
                Ok(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .expect("the gateway notices the back end left");
    assert_eq!(fault(&e), Some(FaultCode::Unavailable), "{e}");
    assert!(!gateway.apps()[0].online);

    // The same back end, restarted, takes its name back without pairing.
    let camera = back_end(home.bound_addr(), key, "camera again").await;
    settles(&camera, |r| *r == Registration::Registered).await;
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "camera again");

    phone.shutdown().await;
    camera.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_stays_with_its_back_end_until_that_one_is_unpaired() {
    let dir = Scratch::new();
    let phones = Pairing::new(PeerSet::new());
    let back_ends = PeerSet::open(dir.0.join("back-ends")).unwrap();
    let gateway = Gateway::open(back_ends.clone(), dir.0.join("apps")).unwrap();
    let home_key = NodeKey::generate();
    let home = gateway_node(&gateway, &phones, home_key.clone()).await;
    let first = paired_back_end(&gateway, &home, NodeKey::generate(), "first").await;
    let phone = phone().await;
    phone
        .pair(&phones.issue(home.bound_addr(), MINUTE))
        .await
        .unwrap();

    // Another back end, paired too, is refused the name.
    let second_key = NodeKey::generate();
    let second = back_end(home.bound_addr(), second_key.clone(), "second").await;
    let ticket = gateway.pairing().issue(home.bound_addr(), MINUTE);
    second.pair_gateway(&ticket).await.unwrap();
    settles(
        &second,
        |r| matches!(r, Registration::Refused(fault) if fault.code == FaultCode::Denied),
    )
    .await;
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "first");

    // The gateway restarts; the name's owner is kept, and so is the refusal.
    // Until the owner is back, phones hear that the app is unavailable, not
    // that the gateway does not know it.
    first.shutdown().await;
    second.shutdown().await;
    home.shutdown().await;
    let gateway = Gateway::open(back_ends.clone(), dir.0.join("apps")).unwrap();
    let home = gateway_node(&gateway, &phones, home_key).await;
    assert_eq!(gateway.apps().len(), 1);
    let e = who_echoes(&phone, &home).await.unwrap_err();
    assert_eq!(fault(&e), Some(FaultCode::Unavailable), "{e}");
    let second = back_end(home.bound_addr(), second_key, "second").await;
    settles(&second, |r| matches!(r, Registration::Refused(_))).await;

    // Unpairing the owner frees the name for the next back end to register.
    let owner = gateway.apps()[0].owner;
    back_ends.remove(&owner).unwrap();
    settles(&second, |r| *r == Registration::Registered).await;
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "second");
    assert_eq!(gateway.apps()[0].owner, second.id());

    phone.shutdown().await;
    second.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unpairing_a_back_end_ends_its_registration_at_once() {
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = gateway_node(&gateway, &phones, NodeKey::generate()).await;
    let camera = paired_back_end(&gateway, &home, NodeKey::generate(), "camera").await;
    let phone = phone().await;
    phone
        .pair(&phones.issue(home.bound_addr(), MINUTE))
        .await
        .unwrap();
    let client = phone.connect(home.bound_addr(), APP).await.unwrap();
    client.call("echo", Value::Null).await.unwrap();

    gateway.pairing().peers().remove(&camera.id()).unwrap();
    let e = client.call("echo", Value::Null).await.unwrap_err();
    assert_eq!(fault(&e), Some(FaultCode::Unavailable), "{e}");
    settles(
        &camera,
        |r| matches!(r, Registration::Refused(fault) if fault.code == FaultCode::Denied),
    )
    .await;

    phone.shutdown().await;
    camera.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_back_end_can_narrow_what_the_gateway_admits() {
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = gateway_node(&gateway, &phones, NodeKey::generate()).await;
    let welcome = phone().await;
    let other = phone().await;
    for phone in [&welcome, &other] {
        phone
            .pair(&phones.issue(home.bound_addr(), MINUTE))
            .await
            .unwrap();
    }
    let camera = Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .serve(APP, echo("camera"))
        .authorizer(Allowlist::new([welcome.id()]))
        .behind_gateway(home.bound_addr())
        .bind()
        .await
        .unwrap();
    let ticket = gateway.pairing().issue(home.bound_addr(), MINUTE);
    camera.pair_gateway(&ticket).await.unwrap();
    settles(&camera, |r| *r == Registration::Registered).await;

    assert_eq!(who_echoes(&welcome, &home).await.unwrap(), "camera");
    let e = who_echoes(&other, &home).await.unwrap_err();
    assert_eq!(fault(&e), Some(FaultCode::Denied), "{e}");

    welcome.shutdown().await;
    other.shutdown().await;
    camera.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_back_end_registering_again_takes_its_requests_off_the_old_connection() {
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = gateway_node(&gateway, &phones, NodeKey::generate()).await;
    let key = NodeKey::generate();
    let old = paired_back_end(&gateway, &home, key.clone(), "old").await;
    let phone = phone().await;
    phone
        .pair(&phones.issue(home.bound_addr(), MINUTE))
        .await
        .unwrap();

    // A stream under way to the old one: `upper` waits for the phone to finish.
    let client = phone.connect(home.bound_addr(), APP).await.unwrap();
    let mut io = client.open("upper", Value::Null).await.unwrap();
    io.send.write_all(b"half").await.unwrap();

    // The same back end on a new connection, as when it restarted after
    // losing its network without the gateway hearing it go.
    let new = back_end(home.bound_addr(), key, "new").await;
    settles(&new, |r| *r == Registration::Registered).await;

    // The stream on the old connection fails now, not at its idle timeout.
    let mut back = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(5), io.recv.read_to_end(&mut back))
        .await
        .expect("the old stream ends at once");
    assert!(
        ended.is_err(),
        "it fails rather than ending cleanly: {back:?}"
    );
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "new");

    old.shutdown().await;
    phone.shutdown().await;
    new.shutdown().await;
    home.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_back_end_with_no_relay_of_its_own_finds_its_gateway_through_the_gateways() {
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new(loopback()));
    let relay = iroh_relay::server::Server::spawn(config)
        .await
        .expect("the relay runs");
    let url: RelayUrl = format!("http://{}", relay.http_addr().unwrap())
        .parse()
        .unwrap();
    let phones = Pairing::new(PeerSet::new());
    let gateway = Gateway::new(PeerSet::new());
    let home = Node::builder()
        .relays(Relays::Custom(vec![url.clone()]))
        .bind_addr(loopback())
        .pairing(phones.clone())
        .gateway(gateway.clone())
        .bind()
        .await
        .unwrap();

    // Its key and its relay, and no address: as for a gateway that moved
    // since.
    let moved = PeerAddr::new(home.id()).with_relay(url);
    let camera = back_end(moved.clone(), NodeKey::generate(), "camera").await;
    camera
        .pair_gateway(&gateway.pairing().issue(moved, MINUTE))
        .await
        .expect("pairs through the relay");
    settles(&camera, |r| *r == Registration::Registered).await;
    let phone = phone().await;
    phone
        .pair(&phones.issue(home.bound_addr(), MINUTE))
        .await
        .unwrap();
    assert_eq!(who_echoes(&phone, &home).await.unwrap(), "camera");

    phone.shutdown().await;
    camera.shutdown().await;
    home.shutdown().await;
    let _ = relay.shutdown().await;
}
