# kuri_link (Dart)

The Dart side of Kuri Link, generated from `crates/kuri-link-ffi` by
[flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge) 2.13.0.
It is a plain Dart package: it works in Flutter apps and in `dart test`, and it
ships no native code of its own (see below).

## Use

```dart
import 'package:kuri_link/kuri_link.dart';

await KuriLinkLib.init(externalLibrary: ExternalLibrary.open(appLibrary));
final node = await LinkNode.start(keyPath: '$dataDir/device.key', relays: []);

// Once, from the text of the back end's QR code. Keep the address.
final PeerAddr backEnd = await node.pair(ticket: scanned);

// Configuration decides; the code after this line is the same either way.
final backend = useLocal ? const Backend.local() : Backend.remote(peer: backEnd);
final client = await node.openApp(app: 'calc', backend: backend, credential: null);

final sum = cbor.decode(await client.call(
    method: 'add', argsCbor: cbor.encode(CborList([CborSmallInt(2), CborSmallInt(3)]))));
```

- `LinkNode.start(keyPath, relays)`: this device's identity and endpoint. The
  key file is created on first start and keeps the device the same across
  launches. `relays` empty means direct paths only.
- `node.id()`: this device's public key, by which a back end knows it once
  paired.
- `node.pair(ticket)`: redeems a back end's pairing ticket and returns the back
  end's `PeerAddr`. A ticket admits one device.
- `node.openApp(app, backend, credential)`: `Backend.local()` or
  `Backend.remote(peer: ...)`.
- `client.call(method, argsCbor)`: the CBOR-encoded result. Encode and decode
  with the [`cbor`](https://pub.dev/packages/cbor) package; empty `argsCbor`
  stands for CBOR null.
- `client.open(method, argsCbor)`: a `LinkStream` to a method the back end
  serves with `Methods::duplex`: `write`, `finish`, `read`, `readToEnd`.
- `client.isLocal()`.

Failures throw `LinkError`. Its `fault` says how the back end answered
(`denied`: pair first; `methodNotFound`; `badRequest`; `app`: the method
failed), and is null when no answer came: a bad argument, an unreachable peer,
a broken stream. On the wire `methodNotFound` is `no_such_method`; Dart
reserves that name.

## One native library per app

Local back ends register in a registry that lives in the `kuri-link` crate as
a static. If the bindings and the back end were two shared libraries, each
would carry its own copy of `kuri-link`, the back end would register in one
registry, the bindings would look in the other, and `Backend.local()` would
find nothing.

So an app builds one library from its own crate:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
kuri-link = { git = "https://github.com/kuriyen/kuri-link", tag = "v0.1.0" }
kuri-link-ffi = { git = "https://github.com/kuriyen/kuri-link", tag = "v0.1.0" }
```

with the Dart package from the same tag:

```yaml
dependencies:
  kuri_link:
    git:
      url: https://github.com/kuriyen/kuri-link
      path: dart
      ref: v0.1.0
```

```rust
// Without this line the bindings' entry points are not exported.
pub use kuri_link_ffi;

#[no_mangle]
pub extern "C" fn my_app_register() {
    kuri_link::register_local("calc", my_methods());
}
```

and calls the registration once at start-up, through `dart:ffi`, before
opening `Backend.local()`:

```dart
DynamicLibrary.open(appLibrary)
    .lookupFunction<Void Function(), void Function()>('my_app_register')();
```

`examples/echo` is such a crate, and `examples/desktop` is a Flutter app built
on it: its `linux/CMakeLists.txt` runs Cargo and bundles the library. An app
with no local back end can use `libkuri_link_ffi.so` from this repository's
build directly.

Generated code from two flutter_rust_bridge crates cannot share one library,
so an app that wants its own Rust API in Dart as well has to generate it
together with this one.

## Regenerate

After any change to `crates/kuri-link-ffi/src/api.rs`:

```sh
cargo install flutter_rust_bridge_codegen@2.13.0 --locked
cd dart && flutter_rust_bridge_codegen generate
```

This rewrites `lib/src/rust/` and `crates/kuri-link-ffi/src/frb_generated.rs`;
commit both. If they fall out of step, `KuriLinkLib.init` fails on the content
hash. The crate and the Dart package pin the same flutter_rust_bridge version,
and must move together.

## Test

From the repository root:

```sh
cargo build -p kuri-link-echo
cd dart && dart test
```

The tests run the same assertions against `echo` linked into the test process
and against `kuri-link-echo-server` in another process, paired by ticket and
reached over iroh on loopback.
