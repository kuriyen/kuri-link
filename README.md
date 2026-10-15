# Kuri Link

Private RPC between an app's front end and its back end — wherever the back end
runs. Peers dial each other **by public key, not by address**, over
[iroh](https://github.com/n0-computer/iroh): same LAN, a direct hole-punched
path through NAT, or a relay that only ever sees ciphertext.

Kuri Link is part of [Kuriyen](https://github.com/kuriyen). Its idea comes from
the Mongol Empire's *örtöö* relay network: messages travel station to station
until they arrive.

> **Status: 0.0.x, experimental.** The wire format may still change. Nothing is
> published to crates.io or pub.dev yet.

## What it gives an app

- **One API, back end anywhere.** A `Client` for a back end linked into the same
  process behaves exactly like one for a back end on another device — same
  calls, same faults. Where the back end runs is configuration.
- **Two operations.** `call` (arguments in, one reply out) and `open` (accept,
  then a raw byte stream both ways — media, files).
- **No ambient access.** Every request passes an `Authorizer` you supply. The
  default refuses everyone. The caller's key is proven by the QUIC/TLS
  handshake, never taken on its word.
- **No silent third parties.** Relays are off unless you name them — your own
  `iroh-relay`, or the public ones, by explicit choice.
- **Evolvable payloads.** CBOR maps with named keys: a newer peer can add
  optional fields without breaking an older one.

## A taste

```rust
use kuri_link::{Allowlist, Client, Methods, Node, Relays, Value};

// Back end
let methods = Methods::new()
    .typed("add", |_caller, (a, b): (i64, i64)| async move { Ok(a + b) });
let server = Node::builder()
    .relays(Relays::None)
    .authorizer(Allowlist::new([phone_key]))
    .serve("calc", methods.clone())
    .bind()
    .await?;

// Front end — the back end over the network…
let remote = phone_node.connect(server.addr(), "calc").await?;
// …or linked into this process. Same calls either way.
let local = Client::in_process(methods);

let sum: i64 = remote.call_typed("add", &(2, 3)).await?;
```

A Dart/Flutter front end binds to `crates/kuri-link-ffi` through
[flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge); see
`dart/README.md`.

## Layout

```
crates/kuri-link      the library: node, client, wire format, hooks
crates/kuri-link-ffi  the plain-typed surface a Dart front end binds to
docs/design.md        wire format and design decisions
dart/                 the Dart package (generated; see its README)
```

## Build and test

```sh
cargo test --workspace     # end-to-end over loopback; no network, no relays
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this work,
as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
