//! The server binary with no arguments: its ticket must carry an address a
//! front end can dial, then pair it and serve it.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use kuri_link::{Node, Relays, Ticket, Value};

/// Stops the server however the test ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_arguments_the_ticket_reaches_the_server() {
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_kuri-link-echo-server"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("server starts"),
    );
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut line)
        .expect("server prints a ticket");
    let ticket: Ticket = line.trim().parse().expect("a ticket");
    assert!(
        !ticket.addr.addrs.is_empty(),
        "bound to every interface, the ticket still names addresses"
    );

    let phone = Node::builder().relays(Relays::None).bind().await.unwrap();
    let addr = phone.pair(&ticket).await.expect("pairs");
    let echo = phone.connect(addr, kuri_link_echo::APP).await.unwrap();
    let hi = Value::Text("hi".into());
    assert_eq!(echo.call("echo", hi.clone()).await.unwrap(), hi);

    phone.shutdown().await;
    drop(server);
}
