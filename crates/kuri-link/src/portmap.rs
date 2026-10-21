//! Asking the home router to forward a back end's port from outside.
//!
//! iroh can map a port by itself, but it lets the router choose the outside
//! port, and a router chooses a new one every time the node starts. Every
//! device paired with the back end keeps its address, so each restart would
//! strand the devices that are away from home. Here the node asks for the
//! outside port to equal its own, which a router grants when no other device
//! holds it. A back end that keeps its port
//! ([`NodeBuilder::port`](crate::NodeBuilder::port)) therefore keeps its
//! public address across restarts; only a new IP from the ISP changes it.
//!
//! NAT-PMP is asked first: one small UDP exchange with the router. UPnP, which
//! more routers speak but less reliably, comes second. PCP, NAT-PMP's
//! successor, is not spoken; most routers that speak it speak one of the
//! other two as well.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use igd_next::aio::tokio::Tokio;
use igd_next::{AddPortError, PortMappingProtocol, SearchOptions};
use iroh::Endpoint;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

type Gateway = igd_next::aio::Gateway<Tokio>;

const NAT_PMP_PORT: u16 = 5351;

/// How long a mapping is asked to last: two hours, as RFC 6886 recommends.
/// It is renewed long before.
const LIFETIME: u32 = 2 * 60 * 60;

/// How often the router is asked again. Each time renews the mapping and
/// reads the router's outside address, so a new IP from the ISP reaches the
/// node's address within this long.
const RECHECK: Duration = Duration::from_secs(2 * 60);

/// NAT-PMP's retransmission schedule (RFC 6886 §3.1) starts at 250 ms and
/// doubles. A router that speaks it answers within milliseconds, so three
/// tries, 1.75 s in all, are enough to conclude that this one does not.
const NAT_PMP_TRIES: u32 = 3;
const NAT_PMP_FIRST_WAIT: Duration = Duration::from_millis(250);

/// How long to look for a UPnP router. igd-next does not always keep to its
/// own timeout, so the search is also cut off from outside.
const UPNP_SEARCH: Duration = Duration::from_secs(1);
const UPNP_SEARCH_CUTOFF: Duration = Duration::from_secs(2);

/// The name a mapping shows in the router's UPnP table.
const DESCRIPTION: &str = "kuri-link";

/// What the router has granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapState {
    /// Still asking for the first time.
    Pending,
    /// The router forwards this outside address to the node.
    Mapped(SocketAddrV4),
    /// No router forwards the node's port.
    Unmapped,
}

/// Keeps the node's port forwarded for as long as it lives, and its outside
/// address among the node's own.
pub(crate) struct PortMapper {
    state: watch::Receiver<MapState>,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl fmt::Debug for PortMapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PortMapper")
            .field("state", &*self.state.borrow())
            .finish()
    }
}

impl PortMapper {
    /// Starts asking the router to forward `port`, the port of the node's
    /// IPv4 socket, bound on `local_ip` or on every interface.
    pub(crate) fn spawn(endpoint: Endpoint, local_ip: Option<Ipv4Addr>, port: u16) -> PortMapper {
        let (state_tx, state) = watch::channel(MapState::Pending);
        let (stop, stop_rx) = oneshot::channel();
        let task = tokio::spawn(run(endpoint, local_ip, port, state_tx, stop_rx));
        PortMapper {
            state,
            stop: Some(stop),
            task,
        }
    }

    /// The outside address, once the router has answered or `deadline` has
    /// passed, whichever comes first.
    pub(crate) async fn settled(&self, deadline: Instant) -> Option<SocketAddrV4> {
        let mut state = self.state.clone();
        let _ =
            tokio::time::timeout_at(deadline, state.wait_for(|s| *s != MapState::Pending)).await;
        let current = *state.borrow();
        match current {
            MapState::Mapped(external) => Some(external),
            MapState::Pending | MapState::Unmapped => None,
        }
    }

    /// Asks the router to drop the mapping, then stops. A router that does
    /// not answer within a second keeps it until it lapses.
    pub(crate) async fn release(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut self.task).await;
    }
}

impl Drop for PortMapper {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Asks the router, renews what it grants every [`RECHECK`], and keeps the
/// endpoint's outside address in step with it.
async fn run(
    endpoint: Endpoint,
    local_ip: Option<Ipv4Addr>,
    port: u16,
    state: watch::Sender<MapState>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut held: Option<Held> = None;
    let mut advertised: Option<SocketAddrV4> = None;
    // When what the router last granted lapses unless renewed.
    let mut lapses = Instant::now();
    loop {
        match request(&mut held, local_ip, port).await {
            Ok((external, lifetime)) => {
                lapses = Instant::now() + lifetime;
                if advertised != Some(external) {
                    if let Some(old) = advertised.take() {
                        endpoint.remove_external_addr(&old.into()).await;
                    }
                    endpoint.add_external_addr(external.into()).await;
                    advertised = Some(external);
                    tracing::info!(%external, "the router forwards this node's port");
                }
                state.send_replace(MapState::Mapped(external));
            }
            Err(e) => {
                // A router that stops answering, say while it restarts, may
                // still forward the port: keep the address until the mapping
                // would have lapsed.
                if advertised.is_none() || Instant::now() >= lapses {
                    if let Some(old) = advertised.take() {
                        endpoint.remove_external_addr(&old.into()).await;
                    }
                    if *state.borrow() != MapState::Unmapped {
                        tracing::info!(reason = %e, "no router forwards this node's port");
                    }
                    state.send_replace(MapState::Unmapped);
                } else {
                    tracing::debug!(reason = %e, "renewing the port mapping failed");
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(RECHECK) => {}
            _ = &mut stop => break,
        }
    }
    if let Some(held) = held {
        held.release().await;
    }
}

/// Renews the mapping held, or else asks for a new one: by NAT-PMP, then by
/// UPnP. Returns the outside address and how long the router keeps it.
async fn request(
    held: &mut Option<Held>,
    local_ip: Option<Ipv4Addr>,
    port: u16,
) -> Result<(SocketAddrV4, Duration), String> {
    if let Some(mapping) = held {
        match mapping.renew().await {
            Ok(granted) => return Ok(granted),
            Err(e) => {
                tracing::debug!(reason = %e, "the router did not renew the mapping; asking anew");
                *held = None;
            }
        }
    }
    let Some(router) = netwatch::interfaces::HomeRouter::new() else {
        return Err("no default gateway".into());
    };
    let IpAddr::V4(gateway) = router.gateway else {
        return Err(format!(
            "the default gateway {} is not IPv4",
            router.gateway
        ));
    };
    let local_ip = local_ip.or(match router.my_ip {
        Some(IpAddr::V4(ip)) => Some(ip),
        _ => None,
    });

    let mut reasons = Vec::new();
    let nat_pmp = SocketAddrV4::new(gateway, NAT_PMP_PORT);
    match nat_pmp_map(nat_pmp, port, port, LIFETIME).await {
        Ok((external, lifetime)) => {
            *held = Some(Held::NatPmp {
                gateway: nat_pmp,
                port,
                external_port: external.port(),
            });
            return Ok((external, lifetime));
        }
        Err(e) => reasons.push(format!("NAT-PMP: {e}")),
    }
    match local_ip {
        Some(ip) => match upnp_map(SocketAddrV4::new(ip, port)).await {
            Ok((mapping, granted)) => {
                *held = Some(mapping);
                return Ok(granted);
            }
            Err(e) => reasons.push(format!("UPnP: {e}")),
        },
        None => reasons.push("UPnP: no local IPv4 address to forward to".into()),
    }
    Err(reasons.join("; "))
}

/// A mapping the router has granted, and how to renew or drop it.
enum Held {
    NatPmp {
        gateway: SocketAddrV4,
        port: u16,
        external_port: u16,
    },
    Upnp {
        gateway: Gateway,
        local: SocketAddrV4,
        external_port: u16,
    },
}

impl Held {
    /// Asks for the same mapping again: renews it, and reads the router's
    /// outside address, which may have changed.
    async fn renew(&self) -> Result<(SocketAddrV4, Duration), String> {
        match self {
            Held::NatPmp {
                gateway,
                port,
                external_port,
            } => nat_pmp_map(*gateway, *port, *external_port, LIFETIME)
                .await
                .map_err(|e| format!("NAT-PMP: {e}")),
            Held::Upnp {
                gateway,
                local,
                external_port,
            } => upnp_add(gateway, *local, *external_port)
                .await
                .map_err(|e| format!("UPnP: {e}")),
        }
    }

    async fn release(self) {
        match self {
            Held::NatPmp { gateway, port, .. } => {
                // Lifetime 0 with no suggested port deletes the mapping
                // (RFC 6886 §3.4).
                let _ = nat_pmp_request(gateway, port, 0, 0).await;
            }
            Held::Upnp {
                gateway,
                external_port,
                ..
            } => {
                let _ = gateway
                    .remove_port(PortMappingProtocol::UDP, external_port)
                    .await;
            }
        }
    }
}

/// Asks the NAT-PMP router at `gateway` to forward outside port `suggested`
/// to this host's UDP `port`, and for its outside IP. A router may grant
/// another outside port when `suggested` is taken.
async fn nat_pmp_map(
    gateway: SocketAddrV4,
    port: u16,
    suggested: u16,
    lifetime: u32,
) -> io::Result<(SocketAddrV4, Duration)> {
    let socket = nat_pmp_socket(gateway).await?;
    let reply = nat_pmp_exchange(&socket, &[0, 0], 0).await?;
    let ip = Ipv4Addr::new(reply[8], reply[9], reply[10], reply[11]);
    let (external_port, granted) = nat_pmp_send_map(&socket, port, suggested, lifetime).await?;
    Ok((
        SocketAddrV4::new(ip, external_port),
        Duration::from_secs(granted.into()),
    ))
}

/// One mapping request, with no question about the outside IP. Used to
/// delete a mapping.
async fn nat_pmp_request(
    gateway: SocketAddrV4,
    port: u16,
    suggested: u16,
    lifetime: u32,
) -> io::Result<(u16, u32)> {
    let socket = nat_pmp_socket(gateway).await?;
    nat_pmp_send_map(&socket, port, suggested, lifetime).await
}

async fn nat_pmp_socket(gateway: SocketAddrV4) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    // Connected, so that only the router's answers are read.
    socket.connect(gateway).await?;
    Ok(socket)
}

/// Sends a UDP mapping request; returns the outside port and lifetime granted.
async fn nat_pmp_send_map(
    socket: &UdpSocket,
    port: u16,
    suggested: u16,
    lifetime: u32,
) -> io::Result<(u16, u32)> {
    let mut request = [0u8; 12];
    request[1] = 1; // map UDP
    request[4..6].copy_from_slice(&port.to_be_bytes());
    request[6..8].copy_from_slice(&suggested.to_be_bytes());
    request[8..12].copy_from_slice(&lifetime.to_be_bytes());
    let reply = nat_pmp_exchange(socket, &request, 1).await?;
    if reply.len() < 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short mapping reply",
        ));
    }
    let external_port = u16::from_be_bytes([reply[10], reply[11]]);
    let granted = u32::from_be_bytes([reply[12], reply[13], reply[14], reply[15]]);
    Ok((external_port, granted))
}

/// Sends `request` until the answer to opcode `op` arrives, and checks its
/// result code.
async fn nat_pmp_exchange(socket: &UdpSocket, request: &[u8], op: u8) -> io::Result<Vec<u8>> {
    let mut wait = NAT_PMP_FIRST_WAIT;
    let mut buf = [0u8; 64];
    for _ in 0..NAT_PMP_TRIES {
        socket.send(request).await?;
        let deadline = Instant::now() + wait;
        loop {
            let n = match tokio::time::timeout_at(deadline, socket.recv(&mut buf)).await {
                Ok(read) => read?,
                Err(_) => break,
            };
            // Version 0, the request's opcode plus 128, a result code.
            if n < 4 || buf[0] != 0 || buf[1] != op + 128 {
                continue;
            }
            return match u16::from_be_bytes([buf[2], buf[3]]) {
                0 if n >= 12 => Ok(buf[..n].to_vec()),
                0 => Err(io::Error::new(io::ErrorKind::InvalidData, "short reply")),
                code => Err(io::Error::other(nat_pmp_result(code))),
            };
        }
        wait *= 2;
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "no answer"))
}

/// What a NAT-PMP result code means (RFC 6886 §3.5).
fn nat_pmp_result(code: u16) -> String {
    let meaning = match code {
        1 => "unsupported version",
        2 => "refused: the router has port mapping turned off",
        3 => "the router has no outside connection",
        4 => "the router has no ports left",
        5 => "unsupported request",
        _ => "unknown result",
    };
    format!("{meaning} (result {code})")
}

/// Finds the UPnP router and asks it to forward outside port `local.port()`
/// to `local`, or any outside port if that one is taken.
async fn upnp_map(local: SocketAddrV4) -> Result<(Held, (SocketAddrV4, Duration)), String> {
    let options = SearchOptions {
        timeout: Some(UPNP_SEARCH),
        ..Default::default()
    };
    let gateway = tokio::time::timeout(
        UPNP_SEARCH_CUTOFF,
        igd_next::aio::tokio::search_gateway(options),
    )
    .await
    .map_err(|_| "no router answered the search".to_string())?
    .map_err(|e| format!("searching for the router: {e}"))?;
    let granted = match upnp_add(&gateway, local, local.port()).await {
        Ok(granted) => granted,
        Err(_) => {
            // Another device holds that outside port.
            let ip = upnp_external_ip(&gateway).await?;
            let external_port = gateway
                .add_any_port(
                    PortMappingProtocol::UDP,
                    local.into(),
                    LIFETIME,
                    DESCRIPTION,
                )
                .await
                .map_err(|e| e.to_string())?;
            (
                SocketAddrV4::new(ip, external_port),
                Duration::from_secs(LIFETIME.into()),
            )
        }
    };
    let held = Held::Upnp {
        gateway,
        local,
        external_port: granted.0.port(),
    };
    Ok((held, granted))
}

/// Asks `gateway` to forward `external_port` to `local`, which also renews a
/// mapping it already holds.
async fn upnp_add(
    gateway: &Gateway,
    local: SocketAddrV4,
    external_port: u16,
) -> Result<(SocketAddrV4, Duration), String> {
    let ip = upnp_external_ip(gateway).await?;
    let add = |lease| {
        gateway.add_port(
            PortMappingProtocol::UDP,
            external_port,
            local.into(),
            lease,
            DESCRIPTION,
        )
    };
    let lifetime = match add(LIFETIME).await {
        Ok(()) => LIFETIME,
        // Some routers keep a mapping only until they restart, with no lease.
        Err(AddPortError::OnlyPermanentLeasesSupported) => {
            add(0).await.map_err(|e| e.to_string())?;
            LIFETIME
        }
        Err(e) => return Err(e.to_string()),
    };
    Ok((
        SocketAddrV4::new(ip, external_port),
        Duration::from_secs(lifetime.into()),
    ))
}

async fn upnp_external_ip(gateway: &Gateway) -> Result<Ipv4Addr, String> {
    match gateway.get_external_ip().await {
        Ok(IpAddr::V4(ip)) => Ok(ip),
        Ok(other) => Err(format!("the router's outside address {other} is not IPv4")),
        Err(e) => Err(format!("asking for the outside address: {e}")),
    }
}

/// Whether devices anywhere on the internet can address `ip`: it is not
/// private, shared (carrier-grade NAT), loopback, link-local, or reserved for
/// some other use.
pub(crate) fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        // Shared address space (RFC 6598): an ISP's carrier-grade NAT.
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 192 && b == 0 && c == 0)
        // Benchmarking (RFC 2544).
        || (a == 198 && (b & 0xfe) == 18)
        || a >= 240)
}

/// Whether `addr` is an IPv4 address devices anywhere can dial.
pub(crate) fn is_public(addr: &SocketAddr) -> bool {
    matches!(addr, SocketAddr::V4(v4) if is_public_v4(*v4.ip()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_means_routable_from_anywhere() {
        for ip in ["1.1.1.1", "8.8.8.8", "100.128.0.1", "198.20.0.1"] {
            assert!(is_public_v4(ip.parse().unwrap()), "{ip} is public");
        }
        for ip in [
            "192.168.1.1",
            "10.0.0.1",
            "172.16.5.4",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.1.1",
            "0.0.0.0",
            "192.0.2.40",
            "192.0.0.8",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!is_public_v4(ip.parse().unwrap()), "{ip} is not public");
        }
    }

    /// A NAT-PMP router on loopback: grants the suggested outside port, or
    /// answers every request with `refuse` when that is set.
    async fn fake_router(
        refuse: Option<u16>,
    ) -> (SocketAddrV4, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let SocketAddr::V4(addr) = socket.local_addr().unwrap() else {
            unreachable!()
        };
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            let mut buf = [0u8; 64];
            while let Ok(Ok((n, from))) =
                tokio::time::timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await
            {
                let request = buf[..n].to_vec();
                let op = request[1];
                let mut reply = vec![0, op + 128];
                reply.extend_from_slice(&refuse.unwrap_or(0).to_be_bytes());
                reply.extend_from_slice(&7u32.to_be_bytes()); // seconds since the epoch
                if op == 0 {
                    reply.extend_from_slice(&[203, 0, 113, 9]);
                } else {
                    let suggested = &request[6..8];
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(if suggested == [0, 0] {
                        &request[4..6]
                    } else {
                        suggested
                    });
                    reply.extend_from_slice(&request[8..12]);
                }
                seen.push(request);
                socket.send_to(&reply, from).await.unwrap();
            }
            seen
        });
        (addr, task)
    }

    #[tokio::test]
    async fn nat_pmp_asks_for_the_nodes_own_port_outside() {
        let (router, seen) = fake_router(None).await;
        let (external, lifetime) = nat_pmp_map(router, 47321, 47321, LIFETIME).await.unwrap();
        assert_eq!(external, "203.0.113.9:47321".parse().unwrap());
        assert_eq!(lifetime, Duration::from_secs(LIFETIME.into()));

        nat_pmp_request(router, 47321, 0, 0).await.unwrap();
        let seen = seen.await.unwrap();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert_eq!(seen[0], [0, 0]);
        let mut map = vec![0, 1, 0, 0];
        map.extend_from_slice(&47321u16.to_be_bytes());
        map.extend_from_slice(&47321u16.to_be_bytes());
        map.extend_from_slice(&LIFETIME.to_be_bytes());
        assert_eq!(seen[1], map);
        // Deleting: lifetime 0 and no suggested port.
        let mut delete = vec![0, 1, 0, 0];
        delete.extend_from_slice(&47321u16.to_be_bytes());
        delete.extend_from_slice(&[0; 6]);
        assert_eq!(seen[2], delete);
    }

    #[tokio::test]
    async fn a_router_that_refuses_says_why() {
        let (router, _seen) = fake_router(Some(2)).await;
        let e = nat_pmp_map(router, 47321, 47321, LIFETIME)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("port mapping turned off"), "{e}");
    }

    #[tokio::test]
    async fn no_router_is_known_within_two_seconds() {
        // Bound, never answers.
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let SocketAddr::V4(addr) = silent.local_addr().unwrap() else {
            unreachable!()
        };
        let start = Instant::now();
        let e = nat_pmp_map(addr, 47321, 47321, LIFETIME).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }
}
