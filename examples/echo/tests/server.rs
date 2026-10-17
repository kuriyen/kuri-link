//! The server binary: its ticket must carry a way to reach it that a front end
//! can use, then pair it and serve it.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use kuri_link::{Node, RelayUrl, Relays, Ticket, Value};

/// Stops the server however the test ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts the server with `args` and reads the ticket it prints.
fn start(args: &[&str]) -> (Server, Ticket) {
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_kuri-link-echo-server"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("server starts"),
    );
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut line)
        .expect("server prints a ticket");
    (server, line.trim().parse().expect("a ticket"))
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_arguments_the_ticket_reaches_the_server() {
    let (server, ticket) = start(&[]);
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

#[tokio::test(flavor = "multi_thread")]
async fn with_a_relay_the_ticket_reaches_the_server_through_it() {
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new((
        std::net::Ipv4Addr::LOCALHOST,
        0,
    )));
    let relay_server = iroh_relay::server::Server::spawn(config)
        .await
        .expect("relay starts");
    let relay: RelayUrl = format!("http://{}", relay_server.http_addr().expect("plain HTTP"))
        .parse()
        .unwrap();

    let (server, mut ticket) = start(&["--bind", "127.0.0.1:0", "--relay", &relay.to_string()]);
    assert_eq!(
        ticket.addr.relays,
        [relay.clone()].into(),
        "the ticket names the relay"
    );

    // Drop the direct address, as if the phone were on another network: the
    // relay is then the only way to the server.
    ticket.addr.addrs.clear();
    let phone = Node::builder()
        .relays(Relays::Custom(vec![relay]))
        .bind()
        .await
        .unwrap();
    let addr = tokio::time::timeout(std::time::Duration::from_secs(30), phone.pair(&ticket))
        .await
        .expect("pairs within 30 seconds")
        .expect("pairs");
    let echo = phone.connect(addr, kuri_link_echo::APP).await.unwrap();
    let hi = Value::Text("hi".into());
    assert_eq!(echo.call("echo", hi.clone()).await.unwrap(), hi);

    phone.shutdown().await;
    drop(server);
    relay_server.shutdown().await.unwrap();
}
