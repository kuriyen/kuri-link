//! The gateway program: its commands issue tickets, show what is paired and
//! registered, and unpair, while it forwards a phone's requests to a back end
//! behind it.

use std::io::{BufRead, BufReader, Read};
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
    let mut gateway = Gateway(
        Command::new(PROGRAM)
            .arg("--dir")
            .arg(dir)
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
