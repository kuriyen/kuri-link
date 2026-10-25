//! The `echo` back end on its own. Prints a pairing ticket, and on a terminal
//! draws it as a QR code for a phone to scan, then serves the devices that
//! redeem one until it is stopped.
//!
//! ```text
//! kuri-link-echo-server [--port PORT | --bind ADDR] [--port-mapping]
//!                       [--peers FILE] [--key FILE] [--relay URL]...
//! kuri-link-echo-server --gateway FILE [--gateway-ticket TICKET] [--key FILE]
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
//!
//! With `--gateway`, the server listens for nothing and issues no ticket:
//! it serves through a home's gateway, such as `kuri-link-gateway`, which
//! pairs the phones and forwards their requests. FILE holds the gateway's
//! address, and the server keeps it current as the gateway reports changes.
//! The first time, pass `--gateway-ticket` too, with a ticket from
//! `kuri-link-gateway pair back-end`: the server pairs with the gateway and
//! writes FILE. Keep `--key` as well, or the gateway sees a stranger after a
//! restart.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::time::Duration;

use std::path::{Path, PathBuf};

use kuri_link::{
    Node, NodeBuilder, Pairing, PeerAddr, PeerSet, Reach, Registration, RelayUrl, Relays, Ticket,
};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;

const TICKET_TTL: Duration = Duration::from_secs(10 * 60);

/// How long to wait for the router to forward the server's port. A home
/// router answers within a second; one that has not answered by now will not.
const MAPPING_WAIT: Duration = Duration::from_secs(5);

/// Why the server exits when its node's endpoint closed by itself: it would
/// stay unreachable, and started again, it binds afresh.
const CLOSED: &str = "the network endpoint closed by itself, as iroh does when the \
                      --bind address goes away and there is no --relay";

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
    let mut peers = None;
    let mut key = None;
    let mut gateway: Option<PathBuf> = None;
    let mut gateway_ticket: Option<Ticket> = None;
    let mut relays: Vec<RelayUrl> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--bind" => bind = Some(value()?.parse().map_err(|e| format!("--bind: {e}"))?),
            "--port" => port = Some(value()?.parse().map_err(|e| format!("--port: {e}"))?),
            "--port-mapping" => port_mapping = true,
            "--peers" => {
                peers = Some(PeerSet::open(value()?).map_err(|e| format!("--peers: {e}"))?)
            }
            "--key" => {
                let path = value()?;
                key = Some(
                    kuri_link::load_or_create_key(path.as_ref())
                        .map_err(|e| format!("--key: {e}"))?,
                );
            }
            "--relay" => relays.push(value()?.parse().map_err(|e| format!("--relay: {e}"))?),
            "--gateway" => gateway = Some(value()?.into()),
            "--gateway-ticket" => {
                gateway_ticket = Some(
                    value()?
                        .parse()
                        .map_err(|e| format!("--gateway-ticket: {e}"))?,
                )
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }

    let mut builder = Node::builder()
        .relays(if relays.is_empty() {
            Relays::None
        } else {
            Relays::Custom(relays.clone())
        })
        .local_discovery(true)
        .serve(kuri_link_echo::APP, kuri_link_echo::methods());
    if let Some(key) = key {
        builder = builder.secret_key(key);
    }
    if let Some(file) = gateway {
        if peers.is_some() || port_mapping || bind.is_some() || port.is_some() {
            return Err(
                "behind a gateway, the gateway pairs the phones and faces outside; \
                        --peers, --port-mapping, --bind and --port are for a server that listens"
                    .into(),
            );
        }
        return behind(builder, &file, gateway_ticket).await;
    }
    if gateway_ticket.is_some() {
        return Err(
            "--gateway-ticket needs --gateway FILE, to keep the gateway's address in".into(),
        );
    }
    let pairing = Pairing::new(peers.unwrap_or_default());
    builder = builder.port_mapping(port_mapping).pairing(pairing.clone());
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
    let closed = node.closed();
    let result = tokio::select! {
        () = stopped() => Ok(()),
        () = closed => Err(CLOSED.to_owned()),
    };
    node.shutdown().await;
    result
}

/// Finishes when the server is told to stop: Ctrl-C, or SIGTERM from a
/// supervisor.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Serves through the gateway whose address `file` holds, pairing with it
/// first if given a ticket.
async fn behind(builder: NodeBuilder, file: &Path, ticket: Option<Ticket>) -> Result<(), String> {
    let gateway: PeerAddr = match &ticket {
        Some(ticket) => ticket.addr.clone(),
        None => std::fs::read_to_string(file)
            .map_err(|e| {
                format!(
                    "--gateway {}: {e}; pair with the gateway first (--gateway-ticket)",
                    file.display()
                )
            })?
            .parse()
            .map_err(|e| format!("--gateway {}: {e}", file.display()))?,
    };
    let node = builder
        .behind_gateway(gateway.clone())
        .bind()
        .await
        .map_err(|e| e.to_string())?;
    let keep = |addr: &PeerAddr| {
        std::fs::write(file, format!("{addr}\n")).map_err(|e| format!("{}: {e}", file.display()))
    };
    if let Some(ticket) = ticket {
        let gateway = node
            .pair_gateway(&ticket)
            .await
            .map_err(|e| format!("pairing with the gateway: {e}"))?;
        keep(&gateway)?;
    }
    eprintln!(
        "serving {} as {} through the gateway {}",
        kuri_link_echo::APP,
        node.id().fmt_short(),
        gateway.id.fmt_short(),
    );
    let mut addr = node.gateway_addr().expect("behind a gateway");
    let mut registration = node.registration().expect("behind a gateway");
    let stop = stopped();
    tokio::pin!(stop);
    let closed = node.closed();
    tokio::pin!(closed);
    let result = loop {
        tokio::select! {
            _ = &mut stop => break Ok(()),
            () = &mut closed => break Err(CLOSED.to_owned()),
            changed = addr.changed() => {
                if changed.is_err() {
                    break Ok(());
                }
                let now = addr.borrow_and_update().clone();
                keep(&now)?;
                eprintln!("the gateway is now at {now}");
            }
            changed = registration.changed() => {
                if changed.is_err() {
                    break Ok(());
                }
                let now = registration.borrow_and_update().clone();
                eprintln!("{}", registered(&now));
            }
        }
    };
    node.shutdown().await;
    result
}

fn registered(registration: &Registration) -> String {
    match registration {
        Registration::Registered => {
            "registered with the gateway: phones paired with it reach this server".into()
        }
        Registration::Refused(fault) => format!("the gateway refused this server: {fault}"),
        _ => "not connected to the gateway; dialing it again".into(),
    }
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
