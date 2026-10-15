//! Pairing end to end over loopback, relays off: tickets, the peer set, and
//! what survives a restart.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use kuri_link::{
    CallError, DenyAll, EndpointAddr, FaultCode, Methods, Node, PairError, Pairing, PeerSet,
    Relays, SecretKey, Ticket, Value, PAIR_APP,
};

const APP: &str = "echo";
const MINUTE: Duration = Duration::from_secs(60);

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

fn echo() -> Methods {
    Methods::new().unary("echo", |_caller, args| async move { Ok(args) })
}

async fn back_end(pairing: &Pairing, key: SecretKey) -> Node {
    Node::builder()
        .secret_key(key)
        .relays(Relays::None)
        .bind_addr(loopback())
        .pairing(pairing.clone())
        .serve(APP, echo())
        .bind()
        .await
        .expect("back end binds")
}

async fn front_end() -> Node {
    Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .bind()
        .await
        .expect("front end binds")
}

fn denied(e: &CallError) -> bool {
    matches!(e, CallError::Fault(fault) if fault.code == FaultCode::Denied)
}

fn refused(e: &PairError) -> bool {
    matches!(e, PairError::Call(e) if denied(e))
}

/// Calls `echo` on the back end at `addr`.
async fn echo_from(node: &Node, addr: EndpointAddr) -> Result<Value, CallError> {
    let client = node.connect(addr, APP).await.expect("connects");
    client.call("echo", Value::Text("hi".into())).await
}

/// A fresh directory under the system's temporary one, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let name = format!("kuri-link-{}", SecretKey::generate().public().fmt_short());
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
async fn a_ticket_pairs_a_device_and_the_pairing_survives_a_restart() {
    let dir = Scratch::new();
    let file = dir.0.join("peers");
    let server_key = SecretKey::generate();
    let phone = front_end().await;

    let pairing = Pairing::new(PeerSet::open(&file).unwrap());
    let server = back_end(&pairing, server_key.clone()).await;
    let e = echo_from(&phone, server.bound_addr()).await.unwrap_err();
    assert!(denied(&e), "unpaired, the phone is refused: {e}");

    // The ticket crosses as text, as it would inside a QR code.
    let text = pairing.issue(server.bound_addr(), MINUTE).to_string();
    let backend = phone.pair(&text.parse().unwrap()).await.expect("pairs");
    assert_eq!(backend.id, server.id());
    assert!(pairing.peers().contains(&phone.id()));
    echo_from(&phone, backend)
        .await
        .expect("paired, it is admitted");
    server.shutdown().await;

    // A new process: the same key, the set read back from its file, and no
    // tickets.
    let pairing = Pairing::new(PeerSet::open(&file).unwrap());
    assert_eq!(pairing.peers().peers(), vec![phone.id()]);
    let server = back_end(&pairing, server_key).await;
    echo_from(&phone, server.bound_addr())
        .await
        .expect("still paired after the restart");

    phone.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_admits_one_device_and_expires() {
    let pairing = Pairing::new(PeerSet::new());
    let server = back_end(&pairing, SecretKey::generate()).await;
    let first = front_end().await;
    let second = front_end().await;

    let ticket = pairing.issue(server.bound_addr(), MINUTE);
    first.pair(&ticket).await.expect("the first device pairs");
    let e = second.pair(&ticket).await.unwrap_err();
    assert!(refused(&e), "a spent ticket is refused: {e}");
    assert_eq!(pairing.peers().peers(), vec![first.id()]);

    let brief = pairing.issue(server.bound_addr(), Duration::from_millis(50));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let e = second.pair(&brief).await.unwrap_err();
    assert!(refused(&e), "an expired ticket is refused: {e}");
    assert_eq!(pairing.peers().len(), 1);

    first.shutdown().await;
    second.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_opens_only_the_pairing_door() {
    let pairing = Pairing::new(PeerSet::new());
    let server = back_end(&pairing, SecretKey::generate()).await;
    let phone = front_end().await;
    let ticket = pairing.issue(server.bound_addr(), MINUTE);
    let token = ticket.token.as_bytes().to_vec();

    // Presented to an ordinary application, the token admits nothing…
    let app = phone.connect(server.bound_addr(), APP).await.unwrap();
    let app = app.with_credential(token.clone());
    assert!(denied(&app.call("echo", Value::Null).await.unwrap_err()));
    // …nor to another method of the pairing application…
    let door = phone.connect(server.bound_addr(), PAIR_APP).await.unwrap();
    let door = door.with_credential(token);
    assert!(denied(&door.call("peers", Value::Null).await.unwrap_err()));
    // …and neither attempt spent it.
    phone.pair(&ticket).await.expect("the ticket still pairs");

    phone.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tickets_can_be_withdrawn_and_devices_unpaired() {
    let pairing = Pairing::new(PeerSet::new());
    let server = back_end(&pairing, SecretKey::generate()).await;
    let phone = front_end().await;

    let one = pairing.issue(server.bound_addr(), MINUTE);
    let two = pairing.issue(server.bound_addr(), MINUTE);
    assert!(pairing.revoke(&one));
    assert!(!pairing.revoke(&one), "already withdrawn");
    assert!(refused(&phone.pair(&one).await.unwrap_err()));
    pairing.revoke_all();
    assert!(refused(&phone.pair(&two).await.unwrap_err()));

    let three = pairing.issue(server.bound_addr(), MINUTE);
    phone.pair(&three).await.expect("pairs");
    let client = phone.connect(server.bound_addr(), APP).await.unwrap();
    client.call("echo", Value::Null).await.expect("admitted");
    assert!(pairing.peers().remove(&phone.id()).unwrap());
    // Authorization runs per request: the connection already open is
    // refused from its next request on.
    assert!(denied(&client.call("echo", Value::Null).await.unwrap_err()));

    phone.shutdown().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_authorizer_decides_for_the_apps_and_pairing_still_works() {
    let pairing = Pairing::new(PeerSet::new());
    let server = Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .authorizer(DenyAll)
        .pairing(pairing.clone())
        .serve(APP, echo())
        .bind()
        .await
        .unwrap();
    let phone = front_end().await;

    let ticket = pairing.issue(server.bound_addr(), MINUTE);
    phone
        .pair(&ticket)
        .await
        .expect("the pairing door has its own guard");
    let e = echo_from(&phone, server.bound_addr()).await.unwrap_err();
    assert!(
        denied(&e),
        "the explicit authorizer decides for the apps: {e}"
    );

    let reserved = Node::builder()
        .relays(Relays::None)
        .bind_addr(loopback())
        .pairing(Pairing::new(PeerSet::new()))
        .serve(PAIR_APP, echo())
        .bind()
        .await;
    assert!(
        reserved.is_err(),
        "the pairing application's name is reserved"
    );

    phone.shutdown().await;
    server.shutdown().await;
}

#[test]
fn ticket_text_fits_a_qr_code_and_reads_back() {
    let addr = EndpointAddr::new(SecretKey::generate().public())
        .with_ip_addr("192.168.86.40:41234".parse().unwrap())
        .with_ip_addr("[fd00::40]:41234".parse().unwrap())
        .with_relay_url("https://relay.example.org".parse().unwrap());
    let ticket = Pairing::new(PeerSet::new()).issue(addr, MINUTE);
    let text = ticket.to_string();

    const QR_ALPHANUMERIC: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
    assert!(text.starts_with("KURI-LINK:"));
    assert!(text.chars().all(|c| QR_ALPHANUMERIC.contains(c)), "{text}");
    assert!(
        format!("{ticket:?}").contains("PairToken(..)"),
        "Debug hides the token"
    );

    let read = |s: &str| s.parse::<Ticket>();
    assert_eq!(read(&text).unwrap(), ticket);
    assert_eq!(read(&text.to_lowercase()).unwrap(), ticket);
    let wrapped = text
        .as_bytes()
        .chunks(32)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(read(&wrapped).unwrap(), ticket, "line breaks are ignored");

    let bare = text.trim_start_matches("KURI-LINK:");
    assert!(read(bare).is_err(), "the prefix is required");
    assert!(read("KURI-LINK:NOT/BASE32").is_err());
    assert!(
        read(&text[..text.len() - 8]).is_err(),
        "a truncated ticket is refused"
    );
}
