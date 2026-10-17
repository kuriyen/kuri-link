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

## 0.1.0 — 2026-10-16

The first tagged release.

### `kuri-link`

- `Node` and `NodeBuilder`: an iroh endpoint that serves apps by name. The key
  comes from a file (`load_or_create_key`). Relays are off unless named
  (`Relays`). Every request passes an `Authorizer`, by default `DenyAll`.
  Optional `Waker` and frame limit.
- `Client`: `call`, `call_typed` and `open`, alike for a back end on another
  device and one in this process (`Client::in_process`, `register_local`).
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
