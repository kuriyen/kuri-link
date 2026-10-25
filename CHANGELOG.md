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
- **The Rust and Dart APIs** follow Cargo's rules for 0.x: a patch release,
  which changes only the last number, does not break them, and a minor
  release, which changes the middle one, may.
- **Text forms** stay readable: a key is 64 hex digits, and a ticket is
  `KURI-LINK:` followed by base32. Peer files and tickets written by 0.1.0 still
  read in later releases.

## Unreleased

### `kuri-link`

- **Fixed: a back end whose WiFi came back took up to 30 seconds more to
  register.** While its network was down, each try to reach the gateway
  failed and doubled the wait before the next, up to 30 seconds, and the
  wait ran on after the network came back. Now a change in the back end's
  own addresses, as when WiFi comes back, dials the gateway at once and
  starts the waits over. After outages of 45 and 90 seconds it registered
  again 0.4 to 1.4 seconds after its address came back, where it took 7 to 8
  and 30 seconds.

## 0.3.2 — 2026-10-25

Upgrade to recover sooner when a connection goes quiet: a request to a back
end that crashed fails after 8 seconds rather than 30, and a back end behind a
gateway registers again within seconds of the gateway crashing or the path to
it dropping out, where it took 15 seconds or more. This was tested on a phone,
at home and over mobile data, and with a back end at home behind its gateway.

One thing to check before upgrading: a request now fails when nothing at all
is heard for 8 seconds while it waits, so a caller that counted on a reply
after a longer outage should try again. The Rust and Dart APIs, wire format 1
and the text forms are as in 0.3.1. The example app now runs on macOS and on
the iOS simulator as well.

### `kuri-link`

- **Changed: connecting to a peer that does not serve the app says so.** The
  error reads `it answered, but does not serve that app in wire format 1`,
  where it gave TLS's `error 120: peer doesn't support any known protocol`.
  Nor does it add any more that only a device on the peer's network can reach
  it: the peer answered.
- **Fixed: a request to a back end that crashed waited about 30 seconds.** A
  back end that crashes or loses power closes nothing, and back on the same
  address it does not know the old connection, so a client's next request
  waited out QUIC's idle timeout before failing. Now a request that hears
  nothing at all from its back end for 8 seconds, not even an acknowledgement,
  fails, and the next request dials again. A slow reply still arrives: the
  keep-alives on the way show the back end is there. So does the reply to a
  request made while the device changes networks, which waits for a path on
  the new network as before: the back end answers iroh's tries of the paths
  there. The cost: a request also fails if no path at all answers for 8
  seconds while it waits, as when WiFi fades on the way out before the phone
  moves to mobile data, where it used to get its reply if the network came
  back within the idle timeout. The connection closes with it, and every
  stream open on it ends too.
- **Fixed: after its gateway crashed, a back end took about 15 seconds to
  register again.** A gateway started in place of one that crashed does not
  know the old connections, so a back end behind it found out only when its
  connection timed out, and until then the gateway answered every request for
  its apps `unavailable`. A back end now sends its gateway a keep-alive every
  second, not every 5, and takes the connection for lost after 4 seconds of
  silence, not 10, so it registers again 5 to 6 seconds after a crash. The
  timeout is the gateway's too: it takes a back end that stops without a word
  for gone after 4 seconds of silence, where it took 10. Both suit a back end
  on the gateway's own network; one that reaches it through a relay, or over
  WiFi that drops out, registers again whenever nothing passes for 3 to 4
  seconds.
- **Fixed: a back end whose connection to its gateway kept going silent
  waited ever longer to register again.** A registration that held less than
  30 seconds counts as a failure, which doubles the wait before the next try,
  up to 30 seconds, so that two back ends sharing a key take the registration
  from each other ever more slowly. It counted a registration whose
  connection went silent too, as on a path through a relay or over WiFi that
  drops out, or when the gateway crashed. Now the back end dials again a
  second after such a loss, however short the registration held. Two back
  ends sharing a key still slow down: the gateway closes the connection of
  the one it took the registration from.

### Dart package

- The tests run on macOS too, against the `.dylib` Cargo builds there.

### Examples

- The example app runs on macOS and on the iOS simulator, and builds for iOS
  devices. A build phase in each Xcode project runs
  `darwin/build_library.sh`, which builds the library with Cargo for the
  architectures Xcode builds, puts it in the app's Frameworks, as a framework
  on iOS, and signs it. The macOS app is sandboxed, with the entitlements to
  dial and to be reached. On iOS the app does not look its back end up by
  mDNS, which needs an entitlement Apple grants on request.
- `Cargo.lock` keeps libc at 0.2.189: netdev, which iroh uses to list network
  interfaces, does not build for iOS against later versions. An app built for
  iOS needs the same pin until a release of netdev builds.

### Documentation

- `docs/integration.md` covers macOS and iOS: building, placing and signing
  the library, the entitlements and the local network permission, and the
  libc pin.

## 0.3.1 — 2026-10-24

Upgrade if a home runs its own relay next to the gateway: the gateway can now
tell the relay whom to carry, so pairing a phone at home is all it takes for
the relay to carry it outside. This was tested on a phone, at home and over
mobile data, through a relay that asked the gateway.

Nothing to check before upgrading. Only the gateway program changes; the Rust
and Dart APIs, wire format 1 and the text forms are as in 0.3.0.

### `kuri-link-gateway`

- `--relay-access ADDR` lets an `iroh-relay` on the same machine ask the
  gateway whom to carry (its `access.http.url`): the gateway, and the phones
  and back ends paired with it, and no one else. Pairing a phone is then all
  it takes for the relay to carry it away from home, and unpairing it stops
  the relay carrying it from its next connection, with no list of keys to
  keep in the relay's config. ADDR must be a loopback address. The relay
  sees a key, never a ticket, so a phone pairs over the home network.
- Fixed: when accepting a connection fails, on the control socket or for the
  relay's check, the gateway waits a second before it tries again, instead
  of trying at once and logging a warning each time round.

## 0.3.0 — 2026-10-23

Upgrade if a home runs more than one back end: one gateway now pairs the
phones once and forwards each request to the back end that serves its app, so
a phone reaches every back end at home through one key. A back end behind the
gateway dials out to it and needs no port, mapping or relay of its own. This
was tested on a phone, at home and over mobile data, with the gateway reached
through a relay.

Two changes can break an app on 0.2.x, so check both before upgrading:

- `FaultCode` has a new code, `Unavailable`, and in Rust it is
  `#[non_exhaustive]`. A Rust `match` on it needs a wildcard arm, and a Dart
  `switch` on it needs a case for `FaultCode.unavailable` or a default.
- In Rust, the error enums `Error`, `CallError`, `PairError` and `WireError`
  are `#[non_exhaustive]` too, so a `match` on one needs a wildcard arm; and
  so is `AuthRequest`, which code outside Kuri Link can no longer build from
  its fields, as a test of an authorizer might have. Dart is not affected.

Peers still speak wire format 1, so peers from earlier releases keep working
with 0.3.0; they read `Unavailable` as `unknown`. A phone app built before
this release reached the back ends through the gateway unchanged.

### `kuri-link`

- **Changed: `FaultCode` is `#[non_exhaustive]`, and has a new code,
  `Unavailable`.** A `match` on it needs a wildcard arm. A peer from before
  this release reads the new code as `unknown`.
- **Changed: the error enums `Error`, `CallError`, `PairError` and
  `WireError` are `#[non_exhaustive]`, and so is `AuthRequest`.** A `match` on
  one of the enums needs a wildcard arm. Outside Kuri Link, an `AuthRequest`
  can no longer be built from its fields, or taken apart without `..`; an
  authorizer reads its fields as before. Later releases can then add errors
  and fields without breaking anyone.
- One gateway for a home: a node built with `NodeBuilder::gateway` pairs the
  phones once, and forwards each request to the back end that registered its
  application there, so a phone reaches every back end at home through one
  key. A back end built with `NodeBuilder::behind_gateway` dials out to the
  gateway, registers its applications, and listens for nothing; it pairs with
  the gateway once, with `Node::pair_gateway` and a ticket from
  `Gateway::pairing`. `Node::registration` says whether the gateway forwards
  to it, and `Node::gateway_addr` follows the gateway's address.
- At the gateway, an application's name belongs to the first back end that
  registered it, until that back end is unpaired; `Gateway::open` keeps the
  owners in a file. Unpairing a back end releases its names, at once or, if
  the gateway is down, when it next starts: the gateway stops accepting them,
  as it does a name no back end registered. A front end is authorized at the
  gateway, and the back end sees it as the caller. While no back end serves an
  application, the gateway answers its requests with
  `FaultCode::Unavailable`. A back end that stops without closing its
  connection is taken for gone after 10 seconds of silence, and one that
  registers again replaces its old connection at once.
  A back end dials the gateway through the relays in the gateway's address
  even with `Relays::None`, so a gateway that moved is found by its key.
- `PeerAddr` has a text form (`Display` and `FromStr`) for keeping an address
  in a file: the key, then each direct address and relay, separated by
  spaces.

### `kuri-link-gateway`

- A new program: a home's gateway on its own. It keeps its key, its port, the
  paired phones and back ends, and who serves each app in one directory.
  `kuri-link-gateway pair phone` and `pair back-end` print a one-time ticket,
  as a QR code on a terminal; `status` lists the apps, phones and back ends,
  and `unpair` removes one. The commands reach the running gateway through a
  socket only its user can open; it refuses a directory that other users can
  open. It listens on every interface unless `--bind` names one address.

### Dart package

- `FaultCode.unavailable`: a gateway answered, and no back end serves the app
  there now.

### Examples

- `kuri-link-echo-server --gateway FILE` serves through a gateway instead of
  listening, and issues no ticket. Given `--gateway-ticket` too, it pairs with
  the gateway first; it keeps the gateway's address in FILE.

## 0.2.0 — 2026-10-22

Upgrade if your app reaches a back end from outside its home: a back end can
now keep its public address across restarts, and tells its front ends that
address and each change of it, so a phone away from home keeps finding it.
Both were tested on a phone, over mobile data and at home.

Two changes in behavior can break an app on 0.1.x, so check both before
upgrading from it:

- No node asks the router for a port mapping unless told to. An app that
  relied on iroh's mapping for direct paths from outside turns it on with
  `NodeBuilder::port_mapping(true)`, or uses a relay.
- Method names starting with `kuri-link.` are reserved. `NodeBuilder::bind`
  refuses a service that lists one, and a request for one no longer reaches
  the service.

Peers still speak wire format 1, so 0.1.x and 0.2.0 peers keep working
together.

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
- **Changed: method names starting with `RESERVED_PREFIX` (`kuri-link.`) are
  kept for Kuri Link.** A request for one never reaches an application's
  `Service`, and `NodeBuilder::bind` refuses a service that lists one among its
  `Service::method_names()`, a new method whose default lists none; `Methods`
  lists its own.
- With `NodeBuilder::local_discovery(true)`, nodes announce and look each
  other up on the local network by mDNS, so a device at home finds its back
  end even after the back end's address there changed. Only a node that
  serves an application announces itself, and everyone on that network then
  sees its key and addresses. Off by default: off, a node sends no mDNS
  packet.
- Connecting to a back end whose every address is on its own network, with
  no relay, gives up after five seconds instead of thirty, and the error says
  that only a device on that network can reach it.

### Dart package

- `LinkClient.backEnd()` and `LinkClient.backEndUpdates()`: the back end's
  address as the client knows it, and its changes, to store in place of the
  one kept from pairing.
- `LinkNode.start(..., localDiscovery: true)` looks back ends up on the local
  network by mDNS. Off when not given.

### Examples

- `kuri-link-echo-server` announces itself on the local network by mDNS, and
  the example app looks it up there; both say that everyone on that network
  can see them. On Android the app takes a multicast lock for it.
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
