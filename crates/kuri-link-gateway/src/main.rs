//! A home's gateway on its own: phones pair with it once and reach every back
//! end behind it, and the back ends dial out to it and listen for nothing.
//!
//! ```text
//! kuri-link-gateway [--dir DIR] [--port PORT] [--bind ADDR] [--relay URL]...
//!                   [--port-mapping] [--local-discovery]
//! kuri-link-gateway [--dir DIR] pair phone|back-end
//! kuri-link-gateway [--dir DIR] status
//! kuri-link-gateway [--dir DIR] unpair KEY
//! ```
//!
//! Without a command, it runs the gateway until it is stopped. DIR holds what
//! the gateway keeps across restarts: its key, its port, the phones and back
//! ends paired with it, and which back end serves each app. It also holds the
//! socket through which the commands reach the running gateway, so only the
//! user who runs the gateway can use them: it refuses a DIR that other users
//! can open. DIR defaults to
//! `$XDG_STATE_HOME/kuri-link-gateway`, or `~/.local/state/kuri-link-gateway`.
//!
//! The gateway listens on every interface, on a port picked when it first
//! starts and kept from then on, so that its address stays the same; `--port`
//! picks one instead. `--bind ADDR` listens on that one address, on the same
//! port: `0.0.0.0` keeps the gateway to IPv4, and `127.0.0.1` leaves it
//! reachable from other machines only through its relays. `--relay`,
//! `--port-mapping` and `--local-discovery` work as for
//! `kuri-link-echo-server`: a relay you run keeps the gateway reachable from
//! outside, the router can be asked to forward its port, and mDNS announces
//! it on the home network, where back ends then find it whatever its address
//! there has become.
//!
//! `pair phone` asks the running gateway for a ticket a phone redeems, and
//! `pair back-end` for one a back end redeems; each pairs one device within
//! ten minutes. The ticket is printed on one line, and on a terminal it is
//! drawn as a QR code as well.
//!
//! `status` lists the apps that back ends registered and whether each is
//! served now, then the paired phones and back ends. `unpair KEY` removes a
//! phone or a back end by the key `status` shows: a phone is refused from its
//! next request, and a back end's registration ends at once and its apps are
//! released.

use std::io::IsTerminal;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use kuri_link::{Gateway, Node, Pairing, PeerAddr, PeerId, PeerSet, Reach, RelayUrl, Relays};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};

const TICKET_TTL: Duration = Duration::from_secs(10 * 60);

/// How long to wait for the router to forward the gateway's port. A home
/// router answers within a second; one that has not answered by now will not.
const MAPPING_WAIT: Duration = Duration::from_secs(5);

/// How long a command has to say what it wants.
const COMMAND_WAIT: Duration = Duration::from_secs(2);

const USAGE: &str = "usage: kuri-link-gateway [--dir DIR] [--port PORT] [--bind ADDR] \
                     [--relay URL]... [--port-mapping] [--local-discovery]\n       \
                     kuri-link-gateway [--dir DIR] pair phone|back-end | status | unpair KEY";

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("kuri-link-gateway: {e}");
        std::process::exit(1);
    }
}

/// How the gateway runs.
#[derive(Default)]
struct Options {
    port: Option<u16>,
    bind: Option<IpAddr>,
    relays: Vec<RelayUrl>,
    port_mapping: bool,
    local_discovery: bool,
}

async fn run() -> Result<(), String> {
    let mut dir = None;
    let mut options = Options::default();
    let mut command = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--port" => options.port = Some(value()?.parse().map_err(|e| format!("--port: {e}"))?),
            "--bind" => options.bind = Some(value()?.parse().map_err(|e| format!("--bind: {e}"))?),
            "--relay" => options
                .relays
                .push(value()?.parse().map_err(|e| format!("--relay: {e}"))?),
            "--port-mapping" => options.port_mapping = true,
            "--local-discovery" => options.local_discovery = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown argument {flag}")),
            word => command.push(word.to_owned()),
        }
    }
    let dir = match dir {
        Some(dir) => dir,
        None => default_dir()?,
    };
    let words: Vec<&str> = command.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => serve(&dir, options).await,
        ["pair", "phone" | "back-end"] => {
            let ticket = ask(&dir, &command.join(" ")).await?;
            let ticket = ticket.trim();
            // One line, so a script or a test can read it.
            println!("{ticket}");
            if std::io::stderr().is_terminal() {
                eprintln!("{}", qr(ticket));
            }
            eprintln!(
                "the ticket above pairs one {} with the gateway within {} minutes",
                if words[1] == "phone" {
                    "phone"
                } else {
                    "back end"
                },
                TICKET_TTL.as_secs() / 60,
            );
            Ok(())
        }
        ["status"] | ["unpair", _] => {
            print!("{}", ask(&dir, &command.join(" ")).await?);
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn default_dir() -> Result<PathBuf, String> {
    let state = match std::env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()) {
        Some(state) => PathBuf::from(state),
        None => {
            let home = std::env::var_os("HOME").ok_or("no --dir, and no HOME to put it in")?;
            PathBuf::from(home).join(".local/state")
        }
    };
    Ok(state.join("kuri-link-gateway"))
}

/// Runs the gateway until it is told to stop.
async fn serve(dir: &Path, options: Options) -> Result<(), String> {
    let at = |name: &str, e: std::io::Error| format!("{}: {e}", dir.join(name).display());
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    private(dir)?;
    let key = kuri_link::load_or_create_key(&dir.join("key")).map_err(|e| at("key", e))?;
    let port = keep_port(&dir.join("port"), options.port).map_err(|e| at("port", e))?;
    let phones = Pairing::new(PeerSet::open(dir.join("phones")).map_err(|e| at("phones", e))?);
    let back_ends = PeerSet::open(dir.join("back-ends")).map_err(|e| at("back-ends", e))?;
    let gateway = Gateway::open(back_ends, dir.join("apps")).map_err(|e| at("apps", e))?;
    let control = listen(dir)?;

    let mut builder = Node::builder()
        .secret_key(key)
        .port(port)
        .relays(if options.relays.is_empty() {
            Relays::None
        } else {
            Relays::Custom(options.relays.clone())
        })
        .port_mapping(options.port_mapping)
        .local_discovery(options.local_discovery)
        .pairing(phones.clone())
        .gateway(gateway.clone());
    if let Some(ip) = options.bind {
        builder = builder.bind_addr(SocketAddr::new(ip, port));
    }
    let node = builder.bind().await.map_err(|e| e.to_string())?;
    let reach = node.reach(MAPPING_WAIT).await;
    let home = Home {
        node,
        phones,
        gateway,
        relays: options.relays,
        outside: match reach {
            Reach::Public(addr) | Reach::BehindNat(addr) => Some(addr),
            _ => None,
        },
    };

    // One line, so a script or a test can read it.
    println!("gateway {} on port {port}", home.node.id());
    let on_loopback = options.bind.is_some_and(|ip| ip.is_loopback());
    eprintln!(
        "{}",
        describe(
            &reach,
            options.port_mapping,
            !home.relays.is_empty(),
            on_loopback
        )
    );
    // A node bound to loopback does not use mDNS.
    if options.local_discovery && !on_loopback {
        eprintln!(
            "announced on this network by mDNS: devices on it can see the gateway's \
             key and addresses, and only paired ones get in"
        );
    }
    eprintln!("kuri-link-gateway pair phone|back-end issues a ticket");
    logging();

    let mut terminate = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    loop {
        tokio::select! {
            accepted = control.accept() => match accepted {
                Ok((stream, _)) => home.answer(stream).await,
                Err(e) => tracing::warn!(error = %e, "a command could not connect"),
            },
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
        }
    }
    let _ = std::fs::remove_file(dir.join("control"));
    home.node.shutdown().await;
    Ok(())
}

/// Refuses `dir` if other users can open it: it holds the gateway's key, and
/// the socket through which `pair phone` gets a ticket into every app.
/// Creating it with mode 0700 protects only a directory created here.
fn private(dir: &Path) -> Result<(), String> {
    let mode = std::fs::metadata(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "{} is open to other users (mode {:o}), and it holds the gateway's key \
             and the socket that issues tickets; chmod 700 it, or pick another --dir",
            dir.display(),
            mode & 0o777
        ));
    }
    Ok(())
}

/// The port in `file`, written there first when `chosen` names one or the
/// file does not exist yet.
fn keep_port(file: &Path, chosen: Option<u16>) -> std::io::Result<u16> {
    let port = match (chosen, std::fs::read_to_string(file)) {
        (Some(port), _) => port,
        (None, Ok(text)) => {
            return text
                .trim()
                .parse()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
        }
        (None, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // A free port, picked by the system and let go.
            std::net::UdpSocket::bind("0.0.0.0:0")?.local_addr()?.port()
        }
        (None, Err(e)) => return Err(e),
    };
    std::fs::write(file, format!("{port}\n"))?;
    Ok(port)
}

/// The socket the commands reach the gateway through, readable by its user
/// alone.
fn listen(dir: &Path) -> Result<UnixListener, String> {
    let path = dir.join("control");
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return Err(format!("a gateway already runs with {}", dir.display()));
    }
    // Left by a gateway that did not stop cleanly.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(listener)
}

/// Sends `command` to the running gateway and returns its answer.
async fn ask(dir: &Path, command: &str) -> Result<String, String> {
    let path = dir.join("control");
    let mut stream = UnixStream::connect(&path)
        .await
        .map_err(|e| format!("no gateway runs with {} ({e})", dir.display()))?;
    let io = |e: std::io::Error| format!("talking to the gateway: {e}");
    stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .map_err(io)?;
    stream.shutdown().await.map_err(io)?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.map_err(io)?;
    match answer.strip_prefix("error: ") {
        Some(e) => Err(e.trim_end().to_owned()),
        None => Ok(answer),
    }
}

/// The running gateway, as the commands see it.
struct Home {
    node: Node,
    phones: Pairing,
    gateway: Gateway,
    relays: Vec<RelayUrl>,
    /// The router's outside address for the gateway, if it forwards one.
    outside: Option<SocketAddr>,
}

impl Home {
    async fn answer(&self, stream: UnixStream) {
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        let mut line = String::new();
        let answer = match tokio::time::timeout(COMMAND_WAIT, read.read_line(&mut line)).await {
            Ok(Ok(_)) => self.command(line.trim()),
            _ => Err("no command".into()),
        };
        let text = answer.unwrap_or_else(|e| format!("error: {e}\n"));
        let _ = write.write_all(text.as_bytes()).await;
    }

    fn command(&self, line: &str) -> Result<String, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["pair", "phone"] => {
                tracing::info!("issued a ticket for a phone");
                Ok(format!("{}\n", self.phones.issue(self.addr(), TICKET_TTL)))
            }
            ["pair", "back-end"] => {
                tracing::info!("issued a ticket for a back end");
                let ticket = self.gateway.pairing().issue(self.addr(), TICKET_TTL);
                Ok(format!("{ticket}\n"))
            }
            ["status"] => Ok(self.status()),
            ["unpair", key] => {
                let key: PeerId = key.parse().map_err(|e| format!("{key}: {e}"))?;
                let unpaired = |set: &PeerSet| set.remove(&key).map_err(|e| e.to_string());
                if unpaired(self.phones.peers())? {
                    tracing::info!(phone = %key.fmt_short(), "unpaired a phone");
                    Ok(format!("unpaired the phone {key}\n"))
                } else if unpaired(self.gateway.pairing().peers())? {
                    tracing::info!(back_end = %key.fmt_short(), "unpaired a back end");
                    Ok(format!("unpaired the back end {key}\n"))
                } else {
                    Err(format!("no phone or back end is paired with the key {key}"))
                }
            }
            _ => Err(format!("unknown command {line:?}")),
        }
    }

    /// Where a ticket says the gateway is.
    fn addr(&self) -> PeerAddr {
        let mut addr = self.node.addr();
        if let Some(outside) = self.outside {
            addr = addr.with_addr(outside);
        }
        // The connection to a relay may not be up yet, and until it is, the
        // node does not list it.
        for relay in &self.relays {
            addr = addr.with_relay(relay.clone());
        }
        addr
    }

    fn status(&self) -> String {
        let addr = self.addr();
        let mut out = format!("gateway {}\n", addr.id);
        for socket in &addr.addrs {
            out.push_str(&format!("  address {socket}\n"));
        }
        for relay in &addr.relays {
            out.push_str(&format!("  relay {relay}\n"));
        }
        let apps = self.gateway.apps();
        out.push_str(&format!("apps ({}):\n", apps.len()));
        for app in apps {
            let state = if app.online { "served" } else { "offline" };
            out.push_str(&format!("  {} {state} by {}\n", app.name, app.owner));
        }
        for (what, set) in [
            ("phones", self.phones.peers()),
            ("back ends", self.gateway.pairing().peers()),
        ] {
            out.push_str(&format!("{what} ({}):\n", set.len()));
            for peer in set.peers() {
                out.push_str(&format!("  {peer}\n"));
            }
        }
        out
    }
}

/// Whether a phone away from home can reach the gateway, and if not, what
/// stands in the way.
fn describe(reach: &Reach, asked: bool, relayed: bool, loopback: bool) -> String {
    if relayed {
        return "reachable from outside through the relays given".into();
    }
    if loopback {
        return "reachable from this machine alone: it listens on loopback, with no \
                relay (--relay)"
            .into();
    }
    const ONLY_HERE: &str = "devices on this network can still pair and connect";
    match reach {
        Reach::Public(public) => format!("reachable from outside at {public}"),
        Reach::BehindNat(outside) => format!(
            "not reachable from outside: the router forwards {outside}, but that \
             address is not public; {ONLY_HERE}"
        ),
        Reach::Unmapped(_) if !asked => format!(
            "not reachable from outside without a relay (--relay) or a forwarded \
             port (--port-mapping); {ONLY_HERE}"
        ),
        Reach::Unmapped(why) => {
            format!("not reachable from outside: the router forwards no port ({why}); {ONLY_HERE}")
        }
        _ => format!("not reachable from outside; {ONLY_HERE}"),
    }
}

/// Logs what the gateway does, registrations and pairings among it, to
/// standard error.
fn logging() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::prelude::*;
    let targets = Targets::new()
        .with_target("kuri_link", LevelFilter::INFO)
        .with_target("kuri_link_gateway", LevelFilter::INFO);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(targets)
        .init();
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
