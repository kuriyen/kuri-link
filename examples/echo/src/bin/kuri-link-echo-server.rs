//! The `echo` back end on its own. Prints a pairing ticket, and on a terminal
//! draws it as a QR code for a phone to scan, then serves the devices that
//! redeem one until it is stopped.
//!
//! ```text
//! kuri-link-echo-server [--port PORT | --bind ADDR] [--port-mapping]
//!                       [--peers FILE] [--key FILE] [--relay URL]...
//! ```
//!
//! The server listens on every interface, on a random port unless `--port`
//! names one, and the ticket lists the addresses of those interfaces. `--bind`
//! listens on one address instead.
//!
//! With `--port-mapping`, the server asks the router to forward its port from
//! outside before it issues the ticket, and waits up to five seconds for an
//! answer: a ticket with the router's public address lets a phone on mobile
//! data pair and connect with no relay. The router then holds an inbound port
//! that leads to this machine until the server stops. Without the flag it asks
//! nothing. Either way, it says whether devices outside can reach it.
//!
//! The server announces itself on the local network by mDNS, so a device at
//! home finds it even after its address there changed. Everyone on that
//! network can see its key and addresses; only paired devices get in.
//!
//! `--peers` keeps the paired devices across restarts, and `--key` keeps the
//! server's own key. With `--port` too, the router forwards the same outside
//! port after a restart, so a front end paired once still finds the server,
//! from home or away.
//!
//! `--relay` names a relay you run, such as `iroh-relay`; repeat it for more
//! than one. The server stays reachable through it, and the ticket names it,
//! so a front end on another network (a phone on mobile data) can reach the
//! server when no direct path exists. Without `--relay`, direct paths only.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::time::Duration;

use kuri_link::{Node, Pairing, PeerSet, Reach, RelayUrl, Relays};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;

const TICKET_TTL: Duration = Duration::from_secs(10 * 60);

/// How long to wait for the router to forward the server's port. A home
/// router answers within a second; one that has not answered by now will not.
const MAPPING_WAIT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("kuri-link-echo-server: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut bind: Option<SocketAddr> = None;
    let mut port = None;
    let mut port_mapping = false;
    let mut peers = PeerSet::new();
    let mut key = None;
    let mut relays: Vec<RelayUrl> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--bind" => bind = Some(value()?.parse().map_err(|e| format!("--bind: {e}"))?),
            "--port" => port = Some(value()?.parse().map_err(|e| format!("--port: {e}"))?),
            "--port-mapping" => port_mapping = true,
            "--peers" => peers = PeerSet::open(value()?).map_err(|e| format!("--peers: {e}"))?,
            "--key" => {
                let path = value()?;
                key = Some(
                    kuri_link::load_or_create_key(path.as_ref())
                        .map_err(|e| format!("--key: {e}"))?,
                );
            }
            "--relay" => relays.push(value()?.parse().map_err(|e| format!("--relay: {e}"))?),
            other => return Err(format!("unknown argument {other}")),
        }
    }

    let pairing = Pairing::new(peers);
    let mut builder = Node::builder()
        .relays(if relays.is_empty() {
            Relays::None
        } else {
            Relays::Custom(relays.clone())
        })
        .port_mapping(port_mapping)
        .local_discovery(true)
        .pairing(pairing.clone())
        .serve(kuri_link_echo::APP, kuri_link_echo::methods());
    if let Some(key) = key {
        builder = builder.secret_key(key);
    }
    match (bind, port) {
        (Some(_), Some(_)) => return Err("--bind and --port both name a port; pass one".into()),
        (Some(addr), None) => builder = builder.bind_addr(addr),
        (None, Some(port)) => builder = builder.port(port),
        (None, None) => {}
    }
    let node = builder.bind().await.map_err(|e| e.to_string())?;
    let reach = node.reach(MAPPING_WAIT).await;

    // A socket bound to every interface names no address a peer can dial;
    // the addresses iroh found on those interfaces do.
    let mut addr = node.bound_addr();
    if addr.addrs.is_empty() {
        addr = node.addr();
    }
    // Listed already when the server listens on every interface, but not
    // when it is bound to one address.
    if let Reach::Public(outside) | Reach::BehindNat(outside) = reach {
        addr = addr.with_addr(outside);
    }
    // The relays go in as given: the connection to them may not be up yet, and
    // until it is, iroh does not list them.
    for relay in relays {
        addr = addr.with_relay(relay);
    }
    if addr.addrs.is_empty() && addr.relays.is_empty() {
        return Err(
            "no address to put in the ticket; pass --bind with this host's address, or --relay"
                .into(),
        );
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
    eprintln!("{}", describe(&reach, port_mapping));
    // A node bound to loopback does not use mDNS.
    if !bind.is_some_and(|addr| addr.ip().is_loopback()) {
        eprintln!(
            "announced on this network by mDNS: devices on it can see this \
             server's key and addresses, and only paired ones get in"
        );
    }
    std::future::pending::<()>().await;
    Ok(())
}

/// Whether a device away from this network can reach the server, and if not,
/// what stands in the way.
fn describe(reach: &Reach, asked: bool) -> String {
    const ONLY_HERE: &str = "devices on this network can still pair and connect";
    match reach {
        Reach::Public(public) => format!("reachable from outside at {public}"),
        Reach::BehindNat(outside) => format!(
            "not reachable from outside: the router forwards {outside}, but that \
             address is not public: the ISP puts this network behind its own NAT \
             (carrier-grade NAT), or a second router stands in front; {ONLY_HERE}"
        ),
        Reach::Unmapped(_) if !asked => format!(
            "not reachable from outside: the router was not asked to forward a \
             port (--port-mapping asks it); {ONLY_HERE}"
        ),
        Reach::Unmapped(why) => {
            format!("not reachable from outside: the router forwards no port ({why}); {ONLY_HERE}")
        }
        _ => format!("not reachable from outside; {ONLY_HERE}"),
    }
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
