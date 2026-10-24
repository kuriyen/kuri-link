//! The gateway program: its commands issue tickets, show what is paired and
//! registered, and unpair, while it forwards a phone's requests to a back end
//! behind it.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use kuri_link::{
    CallError, FaultCode, Methods, Node, NodeKey, Registration, Relays, Ticket, Value,
};

const PROGRAM: &str = env!("CARGO_BIN_EXE_kuri-link-gateway");

/// Stops the gateway however the test ends.
struct Gateway(Child);

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Gateway {
    /// Stops the gateway as a supervisor does, and waits until it has.
    fn stop(mut self) {
        let terminated = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(terminated.success());
        assert!(
            self.0.wait().unwrap().success(),
            "the gateway stops cleanly"
        );
    }
}

/// Starts the gateway on `dir` and reads the line it prints once it serves.
fn start(dir: &Path) -> (Gateway, String) {
    start_with(dir, &[])
}

/// Starts the gateway on `dir` with the options `args`.
fn start_with(dir: &Path, args: &[&str]) -> (Gateway, String) {
    let mut gateway = Gateway(
        Command::new(PROGRAM)
            .arg("--dir")
            .arg(dir)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the gateway starts"),
    );
    let mut line = String::new();
    BufReader::new(gateway.0.stdout.take().unwrap())
        .read_line(&mut line)
        .expect("the gateway says it serves");
    (gateway, line.trim().to_owned())
}

/// Runs a command against the gateway on `dir`: its output, or its error.
fn command(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(PROGRAM)
        .arg("--dir")
        .arg(dir)
        .args(args)
        .stderr(Stdio::piped())
        .output()
        .expect("the command runs");
    if out.status.success() {
        Ok(String::from_utf8(out.stdout).unwrap())
    } else {
        Err(String::from_utf8(out.stderr).unwrap())
    }
}

fn ticket(dir: &Path, who: &str) -> Ticket {
    command(dir, &["pair", who])
        .expect("a ticket")
        .trim()
        .parse()
        .expect("the ticket reads")
}

/// Asks the gateway's access check at `at`, with `method`, whether the relay
/// may carry `key`, as `iroh-relay` asks. Returns the status and the text.
fn relay_check(at: &str, method: &str, key: Option<&str>) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(at).expect("the check listens");
    let key = key.map(|key| format!("X-Iroh-NodeId: {key}\r\n"));
    write!(
        stream,
        "{method} / HTTP/1.1\r\nHost: {at}\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n",
        key.unwrap_or_default()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, text) = response.split_once("\r\n\r\n").expect("an HTTP response");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, text.to_owned())
}

fn carries(at: &str, key: &str) -> bool {
    match relay_check(at, "POST", Some(key)) {
        (200, text) => text == "true",
        other => panic!("{other:?}"),
    }
}

/// A fresh directory under the system's temporary one, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let name = format!("kuri-link-{}", NodeKey::generate().public().fmt_short());
        Scratch(std::env::temp_dir().join(name))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_gateway_pairs_both_sides_and_forwards_between_them() {
    let dir = Scratch::new();
    let (gateway, ready) = start(&dir.0);

    let back_end_ticket = ticket(&dir.0, "back-end");
    let back_end = Node::builder()
        .relays(Relays::None)
        .serve(
            "echo",
            Methods::new().typed("whoami", |caller, (): ()| async move {
                Ok(caller.peer().map(|p| p.to_string()))
            }),
        )
        .behind_gateway(back_end_ticket.addr.clone())
        .bind()
        .await
        .unwrap();
    back_end
        .pair_gateway(&back_end_ticket)
        .await
        .expect("pairs");
    let mut registration = back_end.registration().unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        registration.wait_for(|r| *r == Registration::Registered),
    )
    .await
    .expect("registers in time")
    .unwrap();

    let phone = Node::builder().relays(Relays::None).bind().await.unwrap();
    let gateway_addr = phone
        .pair(&ticket(&dir.0, "phone"))
        .await
        .expect("the phone pairs");
    assert!(ready.starts_with(&format!("gateway {} on port ", gateway_addr.id)));
    let echo = phone.connect(gateway_addr.clone(), "echo").await.unwrap();
    let caller: Option<String> = echo.call_typed("whoami", &()).await.unwrap();
    assert_eq!(caller, Some(phone.id().to_string()));

    let status = command(&dir.0, &["status"]).unwrap();
    assert!(
        status.contains(&format!("echo served by {}", back_end.id())),
        "{status}"
    );
    assert!(
        status.contains(&format!("phones (1):\n  {}", phone.id())),
        "{status}"
    );

    let unpaired = command(&dir.0, &["unpair", &phone.id().to_string()]).unwrap();
    assert!(unpaired.contains("unpaired the phone"), "{unpaired}");
    let e = echo.call("whoami", Value::Null).await.unwrap_err();
    assert!(
        matches!(&e, CallError::Fault(f) if f.code == FaultCode::Denied),
        "{e}"
    );
    assert!(command(&dir.0, &["unpair", &phone.id().to_string()]).is_err());

    // One gateway to a directory.
    let second = Command::new(PROGRAM)
        .arg("--dir")
        .arg(&dir.0)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("already runs"),
        "{second:?}"
    );

    // Stopped, it says goodbye to the back end, which hears it at once.
    gateway.stop();
    assert!(!dir.0.join("control").exists());
    tokio::time::timeout(
        Duration::from_secs(5),
        registration.wait_for(|r| *r != Registration::Registered),
    )
    .await
    .expect("the back end notices the gateway left")
    .unwrap();

    // Restarted, it keeps its key and port, and the back end comes back.
    let (_gateway, again) = start(&dir.0);
    assert_eq!(again, ready);
    tokio::time::timeout(
        Duration::from_secs(40),
        registration.wait_for(|r| *r == Registration::Registered),
    )
    .await
    .expect("registers again in time")
    .unwrap();
    let status = command(&dir.0, &["status"]).unwrap();
    assert!(
        status.contains(&format!("echo served by {}", back_end.id())),
        "{status}"
    );
    assert!(status.contains("phones (0):"), "{status}");
    assert!(
        status.contains(&format!("back ends (1):\n  {}", back_end.id())),
        "{status}"
    );

    phone.shutdown().await;
    back_end.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_gateway_listens_on_one_address_when_told() {
    let dir = Scratch::new();
    let (_gateway, ready) = start_with(&dir.0, &["--bind", "127.0.0.1"]);
    let port = ready.rsplit(' ').next().unwrap();

    let status = command(&dir.0, &["status"]).unwrap();
    let addresses: Vec<&str> = status
        .lines()
        .filter_map(|line| line.trim().strip_prefix("address "))
        .collect();
    assert_eq!(addresses, [format!("127.0.0.1:{port}")], "{status}");

    let phone = Node::builder()
        .relays(Relays::None)
        .bind_addr("127.0.0.1:0".parse().unwrap())
        .bind()
        .await
        .unwrap();
    phone
        .pair(&ticket(&dir.0, "phone"))
        .await
        .expect("the phone pairs over loopback");
    phone.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_gateway_tells_its_relay_to_carry_what_is_paired_and_nothing_else() {
    let dir = Scratch::new();
    let at = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let (_gateway, ready) = start_with(&dir.0, &["--relay-access", &at]);
    let gateway = ready.split(' ').nth(1).unwrap();
    assert!(carries(&at, gateway), "the gateway itself");
    assert!(!carries(&at, &NodeKey::generate().public().to_string()));

    let phone = Node::builder().relays(Relays::None).bind().await.unwrap();
    let id = phone.id().to_string();
    assert!(!carries(&at, &id), "a phone not yet paired");
    phone.pair(&ticket(&dir.0, "phone")).await.unwrap();
    assert!(carries(&at, &id), "a paired phone");
    command(&dir.0, &["unpair", &id]).unwrap();
    assert!(!carries(&at, &id), "an unpaired phone");

    let back_end_ticket = ticket(&dir.0, "back-end");
    let back_end = Node::builder()
        .relays(Relays::None)
        .serve("echo", Methods::new())
        .behind_gateway(back_end_ticket.addr.clone())
        .bind()
        .await
        .unwrap();
    let back_end_id = back_end.id().to_string();
    assert!(!carries(&at, &back_end_id), "a back end not yet paired");
    back_end.pair_gateway(&back_end_ticket).await.unwrap();
    assert!(carries(&at, &back_end_id), "a paired back end");

    // Only the relay's question gets an answer.
    assert_eq!(relay_check(&at, "GET", Some(gateway)).0, 405);
    assert_eq!(relay_check(&at, "POST", None).0, 400);
    assert_eq!(relay_check(&at, "POST", Some("not-a-key")).0, 400);

    phone.shutdown().await;
    back_end.shutdown().await;
}

#[test]
fn the_relay_check_stays_on_this_machine() {
    let dir = Scratch::new();
    let refused = Command::new(PROGRAM)
        .arg("--dir")
        .arg(&dir.0)
        .args(["--relay-access", "192.0.2.1:3341"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("not a loopback address"), "{said}");
    assert!(!dir.0.exists(), "nothing is written");
}

#[test]
fn the_gateway_refuses_a_directory_other_users_can_open() {
    let dir = Scratch::new();
    std::fs::create_dir(&dir.0).unwrap();
    let mode = |bits| std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(bits));
    mode(0o755).unwrap();
    let mut refused = Gateway(
        Command::new(PROGRAM)
            .arg("--dir")
            .arg(&dir.0)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let status = (0..100)
        .find_map(|_| {
            let status = refused.0.try_wait().unwrap();
            if status.is_none() {
                std::thread::sleep(Duration::from_millis(100));
            }
            status
        })
        .expect("the gateway refuses to start");
    assert!(!status.success());
    let mut said = String::new();
    refused
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut said)
        .unwrap();
    assert!(said.contains("chmod 700"), "{said}");
    assert!(!dir.0.join("key").exists(), "nothing is written there");

    mode(0o700).unwrap();
    let (gateway, ready) = start(&dir.0);
    assert!(ready.starts_with("gateway "), "{ready}");
    gateway.stop();
}
