# Changelog

Versions are git tags (`vX.Y.Z`). Nothing is published to crates.io or pub.dev;
depend on a tag.

## Compatibility

- **Wire format 1 is where the compatibility promise starts.** From 0.1.0 on,
  changes within version 1 only add: new optional keys, which older peers
  ignore, and new fault codes, which they read as `unknown`. So peers on
  different releases that speak version 1 keep working together, as a phone app
  and a home server updated on different days must. A change that cannot be
  made that way is a new wire version, with its own ALPN
  (`kuri-link/2/<app>`), which a version 1 peer refuses at the handshake
  instead of misreading.
- **The Rust and Dart APIs** follow Cargo's rules for 0.x: a patch release
  (0.1.x) does not break them, and a minor release (0.2) may.
- **Text forms** stay readable: a key is 64 hex digits, and a ticket is
  `KURI-LINK:` followed by base32. Peer files and tickets written by 0.1.0 still
  read in later releases.

## Unreleased

### `kuri-link`

- **Changed: no node asks the router for a port mapping unless told to.**
  Through 0.1.2, iroh asked the home router by itself, on every node, to
  forward the node's port from outside. Now no node sends the router a
  NAT-PMP, UPnP or PCP request until `NodeBuilder::port_mapping(true)` turns it
  on, because a forwarded port is an inbound port into the home. A node that
  relied on iroh's mapping for direct paths from outside turns it on, or uses a
  relay.
- Turned on, a back end asks the home router itself to forward its port, by
  NAT-PMP or else UPnP, and asks for the outside port to equal its own. iroh's
  mapping, which it replaces on a node that serves an application, let the
  router choose a new outside port at every start, so each restart changed the
  back end's public address. The mapping is renewed every two minutes, which
  also picks up a new public IP from the ISP; `Node::shutdown` releases it. A
  router that grants only permanent mappings, which would outlive the node, is
  not used.
- `NodeBuilder::port(port)` binds the node's sockets, both address families,
  on a port of your choice. A back end that keeps its port keeps its public
  address across restarts.
- `Node::reach(timeout)` says whether devices outside the node's network can
  reach it: `Reach::Public(addr)`, `Reach::BehindNat(addr)` (carrier-grade
  NAT, or a second router) or `Reach::Unmapped(reason)`. `Reach` is
  `#[non_exhaustive]`.
- A back end tells its front ends its address, and each change of it, over a
  reserved method, `ADDR_METHOD` (`kuri-link.addr`), which every application
  it serves answers without its service seeing it. A remote `Client` asks once
  it makes its first request, and dials what it learns from then on.
  `Client::peer_addr()` is the address as the client knows it, and
  `Client::peer_addr_updates()` follows its changes, so an app can keep it. A
  back end from before this release refuses the method, and nothing changes.
- Nodes announce and look each other up on the local network by mDNS, so a
  device at home finds its back end even after the back end's address there
  changed. Only a node that serves an application announces itself.
  `NodeBuilder::local_discovery(false)` turns this off.
- Connecting to a back end whose every address is on its own network, with
  no relay, gives up after five seconds instead of thirty, and the error says
  that only a device on that network can reach it.

### Dart package

- `LinkClient.backEnd()` and `LinkClient.backEndUpdates()`: the back end's
  address as the client knows it, and its changes, to store in place of the
  one kept from pairing.

### Examples

- `kuri-link-echo-server --port-mapping` asks the router to forward its port,
  and waits up to five seconds for the answer before it issues its ticket, so
  the ticket carries the public address. Either way, the server says whether
  devices outside can reach it. `--port` keeps the port across restarts.
- Without `--bind`, the server listens on both address families, so its
  ticket lists its IPv6 addresses too.
- The app stores the back end's address whenever the back end reports a
  change, and closes a client it no longer uses. On Android it holds a
  multicast lock, so that it hears the back end announce itself at home.

## 0.1.2 — 2026-10-20

Upgrade if your app talks to a back end on another device: a client whose
connection is lost now dials again by itself, and an Android app can tell its
node that the network changed. Both were tested on a phone, over WiFi, mobile
data and the switch between them. The rest only adds to the API.

### `kuri-link`

- Fixed: once a remote `Client` loses its connection, as a phone out of signal
  for longer than QUIC's idle timeout does, it dials again before its next
  request. Before, every later request failed until the app connected anew. A
  request under way when the connection is lost still fails and is not sent
  again. `Client::path()` is `None` for a lost connection.
- `Node::network_change()` tells a node that the device's network may have
  changed, so that it looks for paths over the new one at once. Android does
  not let native code see network changes, so there an app calls it from the
  platform's network callback; without it, a switch between WiFi and mobile
  data stalled requests for 15 seconds or more.

### Dart package

- A `LinkClient` dials again once its connection is lost, as `Client` does,
  and its `path()` is `null` meanwhile.
- `LinkNode.networkChanged()`, the same as `Node::network_change()`.

### Examples

- On Android, the app watches the default network in `MainActivity` and calls
  `networkChanged()` when it changes.

### Documentation

- `docs/integration.md`, a step-by-step guide to using Kuri Link in an app.

## 0.1.1 — 2026-10-17

Upgrade if you set `NodeBuilder::bind_addr` to keep a node off some
interfaces: in 0.1.0 a node bound to `127.0.0.1` also listened on `[::]`, so
it was reachable on every IPv6 interface. The rest of this release only adds
to the API.

### `kuri-link`

- `Client::path()` reports the path a remote client's requests use now,
  `Route::Direct(addr)` or `Route::Relay(url)`, with its round-trip time.
  `Route` and `PathInfo` are `#[non_exhaustive]`, so later 0.1.x releases can
  add kinds of path and fields without breaking code that matches on them.
- Fixed: `NodeBuilder::bind_addr` binds only the socket it names. Before, iroh
  still bound the unspecified address of the other family, so a node bound to
  `127.0.0.1` also listened on `[::]`.

### Examples

- `examples/desktop` is now `examples/app`, and runs on Android as well as
  Linux. Its Gradle build runs Cargo through cargo-ndk for arm64-v8a and
  x86_64, as its CMake build does on Linux.
- Pairing by QR code: `kuri-link-echo-server` draws its ticket as a QR code
  on a terminal, and the app on a phone scans it, decoding on the device with
  zxing-cpp and sending nothing to a third party.
- `kuri-link-echo-server --relay URL` keeps the server reachable through a
  relay you run, such as `iroh-relay`, and names it in the ticket, so a front
  end on another network can reach the server when no direct path exists.
- The app starts its node with the relays its back end names: from the ticket
  when it pairs, from the stored address on later launches.
- With a remote back end, the app's **Measure** button makes one call a second
  and logs each one's round-trip time and path as CSV, for testing on real
  networks.

### Dart package

- `ticketBackEnd(ticket)` reads the back end a ticket names without redeeming
  it, so an app can start its node with the back end's relays before pairing.
- `LinkNode.relays()` lists the relays a node started with, and
  `LinkNode.shutdown()` closes it.
- `LinkClient.path()` reports the path a remote client's requests use now,
  `LinkRoute.direct(addr)` or `LinkRoute.relay(url)`, with its round-trip time.

## 0.1.0 — 2026-10-16

The first tagged release.

### `kuri-link`

- `Node` and `NodeBuilder`: an iroh endpoint that serves apps by name. The key
  comes from a file (`load_or_create_key`). Relays are off unless named
  (`Relays`). Every request passes an `Authorizer`: by default `DenyAll`, or,
  with pairing, the devices paired so far. Optional `Waker` and frame limit.
- `Client`: `call`, `call_typed` and `open`, alike for a back end on another
  device and one in this process: `Client::in_process` for a service in hand,
  or `Client::local` for one registered with `register_local`.
- `Methods`: `unary`, `typed` and `duplex` methods.
- Pairing: `Pairing` issues one-time, short-lived `Ticket`s as text for a QR
  code, `Node::pair` redeems one, and `PeerSet` is the paired set: shared,
  mutable, and optionally kept in a file across restarts.
- Its own key and address types: `PeerId`, `PeerAddr`, `NodeKey`, `RelayUrl`.
  No iroh type is in the public API.
- Wire format 1: ALPN `kuri-link/1/<app>`, one request per QUIC stream, CBOR
  frames. See `docs/design.md`.

### `kuri-link-ffi`

- The plain-typed surface for front ends in other languages: `LinkNode`,
  `LinkClient`, `LinkStream`, `Backend`, `PeerAddr`, and `LinkError` with a
  `FaultCode`.

### Dart (`dart/`)

- The package `kuri_link`, generated from `kuri-link-ffi` by
  flutter_rust_bridge 2.13.0. An app ships one native library that holds both
  the bindings and its local back ends; `dart/README.md` explains why.

### Examples

- `examples/echo`: a back end that links into an app, or runs as
  `kuri-link-echo-server` and prints a pairing ticket.
- `examples/desktop`: a Flutter app for Linux on that back end, in-process or
  remote, with the same code either way.
