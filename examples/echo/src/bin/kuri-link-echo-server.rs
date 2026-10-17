//! The `echo` back end on its own. Prints a pairing ticket, and on a terminal
//! draws it as a QR code for a phone to scan, then serves the devices that
//! redeem one until it is stopped.
//!
//! ```text
//! kuri-link-echo-server [--bind ADDR] [--peers FILE] [--key FILE]
//! ```
//!
//! `--bind` defaults to every interface on a random port, and the ticket then
//! lists the addresses of those interfaces. `--peers` keeps the paired devices
//! across restarts, and `--key` keeps the server's own key, so a front end
//! paired once still finds it.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::time::Duration;

use kuri_link::{Node, Pairing, PeerSet, Relays};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;

const TICKET_TTL: Duration = Duration::from_secs(10 * 60);

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("kuri-link-echo-server: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut bind: SocketAddr = ([0, 0, 0, 0], 0).into();
    let mut peers = PeerSet::new();
    let mut key = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--bind" => bind = value()?.parse().map_err(|e| format!("--bind: {e}"))?,
            "--peers" => peers = PeerSet::open(value()?).map_err(|e| format!("--peers: {e}"))?,
            "--key" => {
                let path = value()?;
                key = Some(
                    kuri_link::load_or_create_key(path.as_ref())
                        .map_err(|e| format!("--key: {e}"))?,
                );
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }

    let pairing = Pairing::new(peers);
    let mut builder = Node::builder()
        .relays(Relays::None)
        .bind_addr(bind)
        .pairing(pairing.clone())
        .serve(kuri_link_echo::APP, kuri_link_echo::methods());
    if let Some(key) = key {
        builder = builder.secret_key(key);
    }
    let node = builder.bind().await.map_err(|e| e.to_string())?;

    // A socket bound to every interface names no address a peer can dial;
    // the addresses iroh found on those interfaces do.
    let mut addr = node.bound_addr();
    if addr.addrs.is_empty() {
        addr = node.addr();
    }
    if addr.addrs.is_empty() {
        return Err(format!(
            "no address to put in the ticket for {bind}; pass --bind with this host's address"
        ));
    }

    // One line, so a script or a test can read it.
    let ticket = pairing.issue(addr, TICKET_TTL).to_string();
    println!("{ticket}");
    if std::io::stderr().is_terminal() {
        eprintln!("{}", qr(&ticket));
    }
    eprintln!(
        "serving {} as {}; the ticket above pairs one device and expires in {} minutes",
        kuri_link_echo::APP,
        node.id().fmt_short(),
        TICKET_TTL.as_secs() / 60,
    );
    std::future::pending::<()>().await;
    Ok(())
}

/// `text` as a QR code in half-height blocks, two modules to a character,
/// drawn black on white whatever the terminal's own colours are, so a camera
/// reads it the right way round.
fn qr(text: &str) -> String {
    let code = match QrCode::new(text) {
        Ok(code) => code,
        Err(e) => return format!("(no QR code: {e})"),
    };
    code.render::<Dense1x2>()
        .quiet_zone(true)
        .build()
        .lines()
        .map(|line| format!("\x1b[30;107m{line}\x1b[0m\n"))
        .collect()
}
