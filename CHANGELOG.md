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
