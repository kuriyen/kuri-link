# Using Kuri Link in an app

This guide puts Kuri Link into a Flutter app of your own: a Dart front end,
and a Rust back end that runs in the app's process, on another device, or
either, chosen at run time. It goes step by step and points at the files in
this repository that already do each step; `examples/app` is a complete app
built this way. The Dart API is described in detail in
[`dart/README.md`](../dart/README.md), and the wire format and security model
in [`design.md`](design.md).

## What runs where

- **The front end is Dart**: the `kuri_link` package. It starts this device's
  node, pairs with a back end, opens an application on it, and makes calls and
  opens streams.
- **The back end is Rust**: a `Methods` value (or your own `Service`) per
  application. The same value serves both placements:
  - *in the app's process*: your app's native library registers it with
    `register_local`, and the front end opens `Backend.local()`;
  - *on another device*: a Rust program you run serves it from a `Node`, and
    the front end opens `Backend.remote(peer: ...)`.
- **Serving, pairing and authorization are Rust only.** From Dart a node can
  call a back end, not serve one, and it cannot issue pairing tickets.

## Platforms

| Platform | State |
|---|---|
| Linux desktop, x86_64 | Works. `examples/app` builds and runs, and its integration test passes with the back end in process and in another process over loopback. |
| Android, arm64-v8a and x86_64 | Works. `examples/app` builds for both ABIs, and has been tested on a phone (WiFi, mobile data, through a relay, switching between networks) and on the emulator. |
| macOS, iOS, Windows | Not yet. Nothing here builds or bundles the native library for them, and nothing has been tested on them. |
| Web | Not yet. A browser cannot open raw UDP, so a web front end could only ever reach its back end through a relay. |

## Versions

Kuri Link is not on crates.io or pub.dev. Depend on a git tag, and name the
same one in Cargo and in pub:

```toml
[dependencies]
kuri-link = { git = "https://github.com/kuriyen/kuri-link", tag = "v0.1.2" }
kuri-link-ffi = { git = "https://github.com/kuriyen/kuri-link", tag = "v0.1.2" }
```

```yaml
dependencies:
  kuri_link:
    git:
      url: https://github.com/kuriyen/kuri-link
      path: dart
      ref: v0.1.2
```

The Dart package's generated code and the Rust side each carry a hash of the
API they were generated from, and `KuriLinkLib.init` fails when the two
differ. So Cargo and pub must name the same version. Both sides also pin
flutter_rust_bridge to exactly 2.13.0; an app cannot use another version of it
alongside Kuri Link.

The compatibility promise is in [`CHANGELOG.md`](../CHANGELOG.md): from 0.1.0
on, peers that speak wire format 1 keep working together across releases, so
a phone app and a back end can be updated on different days; the Rust and
Dart APIs follow Cargo's 0.x rules, so 0.1.x releases do not break them and
0.2 may.

## 1. Make one native library

An app ships exactly one native library, holding both the Kuri Link bindings
and its in-process back ends. In-process back ends register in a registry
inside the `kuri-link` crate; a second library would carry a second registry,
and `Backend.local()` would find nothing in it. [`dart/README.md`](../dart/README.md)
("One native library per app") has the details.

Make it a crate of your own, with the dependencies above and:

```toml
[lib]
# cdylib: the app's native library. lib: so that a server program (step 4)
# can depend on the crate and serve the same methods.
crate-type = ["lib", "cdylib"]
```

```rust
use kuri_link::Methods;

// Without this line the bindings' entry points are not exported.
pub use kuri_link_ffi;

pub const APP: &str = "calc";

pub fn methods() -> Methods {
    Methods::new().typed("add", |_caller, (a, b): (i64, i64)| async move { Ok(a + b) })
}

/// Called once from Dart at start-up, before `Backend.local()` is opened.
#[no_mangle]
pub extern "C" fn my_app_register() {
    kuri_link::register_local(APP, methods());
}
```

`Methods` has three kinds of method: `unary` takes and returns CBOR values,
`typed` converts arguments and result through serde, and `duplex` takes over
a raw byte stream once it accepts. `examples/echo/src/lib.rs` has one of each.

- An app with no in-process back end still needs a library that exports the
  bindings: the crate above without `register_local` does, and so does
  `libkuri_link_ffi.so` as this repository builds it.
- An app with a Rust API of its own to call from Dart through
  flutter_rust_bridge cannot put it in a second generated library: two
  generated crates cannot share one library, so it has to be generated
  together with this one.

## 2. Build it with the app

Each platform's Flutter build runs Cargo and puts the library where the app
loads it from. Copy the steps from `examples/app`:

- **Linux.** `linux/CMakeLists.txt` adds a target that runs
  `cargo build --lib` for the crate, and installs the `.so` into the bundle's
  `lib/` directory, next to the Flutter engine. The app loads it from there.
- **Android.** `android/app/build.gradle.kts` runs `cargo ndk` for arm64-v8a
  and x86_64 before Gradle merges the JNI libraries, and copies only your
  library into them: the same Cargo build also leaves `libkuri_link_ffi.so`
  in the target directory, which the app does not need. The loader then finds
  the library by name. It needs the Android targets and
  [cargo-ndk](https://github.com/bbqsrc/cargo-ndk):

  ```sh
  rustup target add aarch64-linux-android x86_64-linux-android
  cargo install cargo-ndk
  ```

At start-up, load the library, register the in-process back end, and start
this device's node:

```dart
import 'dart:ffi';
import 'dart:io';

import 'package:kuri_link/kuri_link.dart';

String nativeLibrary() => Platform.isAndroid
    ? 'libmy_app_native.so'
    : '${File(Platform.resolvedExecutable).parent.path}/lib/libmy_app_native.so';

Future<LinkNode> startLink(String dataDir, List<String> relays) async {
  final library = nativeLibrary();
  await KuriLinkLib.init(externalLibrary: ExternalLibrary.open(library));
  DynamicLibrary.open(library).lookupFunction<Void Function(), void Function()>(
    'my_app_register',
  )();
  return LinkNode.start(keyPath: '$dataDir/device.key', relays: relays);
}
```

The key file is created on first start and is this device's identity: every
back end it pairs with knows it by that key. Keep it in the app's private
storage (`examples/app` uses `getApplicationSupportDirectory()` from
`path_provider`) and keep it across updates, or the device has to pair again.
One node per app process is enough.

## 3. Choose the back end

Where the back end runs is configuration. The code that picks it is the only
code that differs:

```dart
final backend = useLocal
    ? const Backend.local()
    : Backend.remote(peer: pairedBackEnd);
final client = await node.openApp(app: 'calc', backend: backend, credential: null);
```

Everything after that line is the same for both. Arguments and results are
CBOR, encoded and decoded with the [`cbor`](https://pub.dev/packages/cbor)
package; empty arguments stand for CBOR null:

```dart
import 'dart:convert';
import 'dart:typed_data';

import 'package:cbor/cbor.dart';

Future<int> add(LinkClient client, int a, int b) async {
  final args = CborList([CborSmallInt(a), CborSmallInt(b)]);
  final out = await client.call(
    method: 'add',
    argsCbor: Uint8List.fromList(cbor.encode(args)),
  );
  return (cbor.decode(out) as CborInt).toInt();
}

// A method served with `Methods::duplex`: write, finish, then read the reply.
Future<String> upper(LinkClient client, String text) async {
  final io = await client.open(method: 'upper', argsCbor: Uint8List(0));
  await io.write(data: utf8.encode(text));
  await io.finish();
  return utf8.decode(await io.readToEnd());
}
```

`EchoPanel` in `examples/app/lib/main.dart` is a widget written this way: it
gets a `LinkClient` and never learns where the back end runs.

A request that fails throws `LinkError`. Its `fault` is set when the back end
answered:

| `fault` | Meaning |
|---|---|
| `denied` | The back end does not admit this device: pair first, or pair again. |
| `methodNotFound` | The application serves no method by that name. |
| `badRequest` | The arguments did not fit the method. |
| `app` | The method ran and failed. |
| `unsupportedVersion` | The back end does not speak this wire version. |
| `unknown` | A code from a newer back end. |
| `null` | No answer came: a bad argument on this side, an unreachable back end, a broken stream. |

## 4. Serve it from another device

A remote back end is a Rust program with a `Node` that serves the same
`Methods` and lets devices pair:

```rust
use std::path::Path;
use std::time::Duration;

use kuri_link::{load_or_create_key, Node, Pairing, PeerSet, Relays};

let pairing = Pairing::new(PeerSet::open("paired-devices")?);
let node = Node::builder()
    .secret_key(load_or_create_key(Path::new("server.key"))?)
    .relays(Relays::None)
    .pairing(pairing.clone())
    .serve(my_app_native::APP, my_app_native::methods())
    .bind()
    .await?;

// Show this as a QR code, next to the back end, when a device asks to pair.
let ticket = pairing.issue(node.addr(), Duration::from_secs(120));
println!("{ticket}");
```

- **Keep the key and the paired set in files**, as above, so a device paired
  once still finds the back end, and is still admitted, after a restart.
- **Who may call.** With `pairing`, the node's applications admit the paired
  devices and no one else; the caller's key is proven by the handshake.
  `NodeBuilder::authorizer` replaces that with your own `Authorizer`, which
  also sees the `credential` bytes a front end passes to `openApp`. Without
  either, the node refuses everyone.
- **Unpairing** is `pairing.peers().remove(&id)`, and takes effect on the
  device's next request, even on a connection already open.

`examples/echo/src/bin/kuri-link-echo-server.rs` is a complete server: it
takes the key, paired set and relays as arguments, puts its relays in the
ticket, and draws the ticket as a QR code on a terminal.

## 5. Pair

A ticket is the back end's address (its key, its sockets, its relays) and a
one-time token, as text: `KURI-LINK:` followed by base32. It admits one
device, expires (at most a day after it is issued; keep it to a minute or
two), and lives only in the back end's memory, so a restart withdraws it.
Until it is redeemed it is a secret: whoever presents it first pairs.

On the front end:

```dart
// Read the back end's address without redeeming the ticket, and start the
// node with the relays it names (see Relays below).
final named = ticketBackEnd(ticket: text);
node = await restartWithRelays(node, dataDir, named.relays);

final PeerAddr paired = await node.pair(ticket: text);
save(paired); // id, addrs and relays: everything needed to reach it later
```

On later launches, start the node with the stored address's relays and open
`Backend.remote(peer: stored)`; no ticket is needed again. `PeerStore` in
`examples/app/lib/main.dart` keeps the address as JSON.

Pairing goes one way: the back end learns the device's key, and the device
keeps the back end's address. Two nodes that serve each other pair twice.

**QR codes.** Any decoder works; the ticket's text uses only characters of the
QR alphanumeric mode, and reading it back ignores case and whitespace. Accept
only text that starts with `KURI-LINK:`. `examples/app` decodes on the device
with `flutter_zxing`, so the image never leaves the phone, and its
`AndroidManifest.xml` removes the microphone and storage permissions that the
camera plugins would otherwise add.

## Relays

A relay forwards packets between two nodes that cannot reach each other
directly, such as a phone on mobile data and a back end behind a home router.
The packets are encrypted end to end: a relay sees which keys talk to each
other, when, and how much, never what they say.

Kuri Link uses no relay unless you name one:

- **None.** `LinkNode.start(relays: [])` and `Relays::None`: direct paths
  only, on the same network or to the addresses a peer's address lists.
- **Your own.** A list of URLs, `Relays::Custom(urls)` in Rust: relays you
  run, such as [`iroh-relay`](https://crates.io/crates/iroh-relay). On the
  back end, also add each one to the address in the ticket
  (`addr.with_relay(url)`): right after `bind`, iroh does not yet list a relay
  it has not finished connecting to.
- **Public.** In Rust only, `Relays::PublicDefault` selects the public relays
  that the iroh maintainers run. That is a third party; choose it explicitly
  or not at all.

A node uses only the relays it started with. To reach a back end through the
relays it names, start the node with them: `ticketBackEnd(ticket:).relays`
before pairing, the stored address's `relays` after. To change relays, shut
the node down and start it again from the same key file; it is still the same
device to every back end:

```dart
Future<LinkNode> restartWithRelays(
  LinkNode node,
  String dataDir,
  List<String> relays,
) async {
  final have = node.relays().toSet();
  if (have.length == relays.length && have.containsAll(relays)) return node;
  final started = await LinkNode.start(
    keyPath: '$dataDir/device.key',
    relays: relays,
  );
  await node.shutdown();
  return started;
}
```

A relay that ships with Kuri Link is planned. Until then, run `iroh-relay`.

## Network changes and lost connections

**Android needs a hint when the network changes.** Native code there cannot
watch the network, so without help a switch between WiFi and mobile data
stalls requests until the old path times out, which took 15 to 17 seconds
on a phone. Watch the default network from the activity and pass each change
to Dart:

```kotlin
import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.os.Handler
import android.os.Looper
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "my_app/network")
        val main = Handler(Looper.getMainLooper())
        val changed = { main.post { channel.invokeMethod("changed", null) } }
        val connectivity = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        connectivity.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) { changed() }
            override fun onLost(network: Network) { changed() }
            override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) { changed() }
        })
    }
}
```

```dart
const MethodChannel('my_app/network')
    .setMethodCallHandler((_) => node.networkChanged());
```

The full version, which also unregisters the callback, is
`examples/app/android/app/src/main/kotlin/io/github/kuriyen/kuri_link_example/MainActivity.kt`.
On Linux iroh notices network changes by itself; calling `networkChanged()`
there does no harm.

**A lost connection is dialed again.** When a remote client's connection is
lost, as a phone's is when it is out of signal for longer than QUIC's idle
timeout, the client dials again before its next request. A request already
under way when the connection is lost fails and is not sent again, since it
may already have taken effect; whether to retry it is the app's decision.

`client.path()` reports the path requests use now, `LinkRoute.direct` or
`LinkRoute.relay`, with its round-trip time; it is `null` in process and while
there is no connection. The **Measure** button in `examples/app` logs it once
a second, which is a quick way to see how an app behaves on a real network.

## Not yet

- **No discovery.** A remote back end is reached at the addresses its ticket
  listed, plus its relays. If its address on the local network changes and it
  has no relay, the device cannot find it until it pairs again.
- **No typed Dart clients.** Dart calls methods by name with CBOR bytes.
- **No macOS, iOS or Windows builds**, as above.
