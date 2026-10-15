# Dart package (not generated yet)

The Dart side is generated from `crates/kuri-link-ffi` with
[flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge) v2:

```sh
cargo install flutter_rust_bridge_codegen
flutter_rust_bridge_codegen integrate      # once, inside a Flutter package here
flutter_rust_bridge_codegen generate       # after every change to kuri-link-ffi
```

The package is `kuri_link` (pub package names use underscores). Surface it
exposes (from `kuri-link-ffi`):

- `LinkNode.start(keyPath, relays)` — this device's identity and endpoint.
- `node.id()` — the public key to put in a pairing QR code.
- `node.openApp(app, backend, credential)` — `Backend.local()` or
  `Backend.remote(peer: PeerAddr(...))`. The choice is configuration; the
  front-end code after this line is identical.
- `client.call(method, argsCbor)` → result CBOR. Encode and decode with the
  `cbor` package.
- `client.isLocal()`.

Local back ends are Rust code linked into the app and registered with
`kuri_link::register_local(app, service)` during start-up.
