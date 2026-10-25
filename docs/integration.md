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
| Android, arm64-v8a and x86_64 | Works. `examples/app` builds for both ABIs, and has been tested on a phone (WiFi, mobile data, through a relay, switching between networks; from outside the home with no relay, after the back end restarted and after it moved to another port) and on the emulator. |
| macOS, Apple silicon | Works. The Rust tests and the Dart package's tests pass, and `examples/app` builds, for both architectures in release, and runs; its integration test passes with the back end in process and in another process over loopback. |
| iOS | Runs on the simulator. `examples/app` builds for devices and the simulator, and its integration test passes on the simulator with the back end in process and in another process over loopback; no device has run it yet. Needs libc pinned for now (step 2). |
| Windows | Not yet. Nothing here builds or bundles the native library for it, and nothing has been tested on it. |
| Web | Not yet. A browser cannot open raw UDP, so a web front end could only ever reach its back end through a relay. |

## Versions

Kuri Link is not on crates.io or pub.dev. Depend on a git tag, and name the
same one in Cargo and in pub:

```toml
[dependencies]
kuri-link = { git = "https://github.com/kuriyen/kuri-link", tag = "v0.3.2" }
kuri-link-ffi = { git = "https://github.com/kuriyen/kuri-link", tag = "v0.3.2" }
```

```yaml
dependencies:
  kuri_link:
    git:
      url: https://github.com/kuriyen/kuri-link
      path: dart
      ref: v0.3.2
```

The Dart package's generated code and the Rust side each carry a hash of the
API they were generated from, and `KuriLinkLib.init` fails when the two
differ. So Cargo and pub must name the same version. Both sides also pin
flutter_rust_bridge to exactly 2.13.0; an app cannot use another version of it
alongside Kuri Link.

The compatibility promise is in [`CHANGELOG.md`](../CHANGELOG.md): from 0.1.0
on, peers that speak wire format 1 keep working together across releases, so
a phone app and a back end can be updated on different days; the Rust and
Dart APIs follow Cargo's 0.x rules, so 0.3.x releases do not break them and
0.4 may.

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
- **macOS and iOS.** A build phase of the Runner target in each Xcode project
  runs `darwin/build_library.sh`. It runs `cargo build --lib` for each
  architecture Xcode builds, joins the results with `lipo`, names the library
  by `@rpath`, puts it in the app's Frameworks and signs it with the app's
  identity. iOS loads code only from frameworks, so there the library is the
  binary of a framework with an `Info.plist` of its own. It needs the targets:

  ```sh
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
  ```

  Run Cargo as a terminal would, not with Xcode's environment as it stands.
  Xcode's clang finds an SDK only through `SDKROOT`, which names the app's
  platform and is wrong for the build scripts Cargo compiles to run on the
  Mac; and an iOS build sets `MACOSX_DEPLOYMENT_TARGET` to the SDK's version,
  which can be newer than the Mac. Keep the deployment target of the platform
  being built: rustc and the C compiler must agree on it, or the link fails.

  **For iOS, pin libc for now.** netdev, which iroh uses to list network
  interfaces, does not build for iOS against libc 0.2.190 or later, which
  declares the `in6_ifreq` it uses on macOS only. Until a release of netdev
  builds, keep libc at 0.2.189 in the app's lockfile, as this repository does:

  ```sh
  cargo update -p libc --precise 0.2.189
  ```

At start-up, load the library, register the in-process back end, and start
this device's node:

```dart
import 'dart:ffi';
import 'dart:io';

import 'package:kuri_link/kuri_link.dart';

String nativeLibrary() {
  final dir = File(Platform.resolvedExecutable).parent.path;
  if (Platform.isAndroid) return 'libmy_app_native.so';
  if (Platform.isMacOS) return '$dir/../Frameworks/libmy_app_native.dylib';
  if (Platform.isIOS) {
    return '$dir/Frameworks/my_app_native.framework/my_app_native';
  }
  return '$dir/lib/libmy_app_native.so';
}

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

**Apple's permissions.** A sandboxed macOS app needs
`com.apple.security.network.client` and `com.apple.security.network.server`
in its entitlements, Debug and Release alike, or its node can neither dial nor
be reached. iOS, and macOS since version 15, ask the user before an app uses
the local network; say why in `Info.plist` under
`NSLocalNetworkUsageDescription`. Loopback needs no permission.

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
    .port(stored_port) // chosen once, kept like the key: see below
    .port_mapping(owner_chose_it) // opens a port on the router: see below
    .relays(Relays::None)
    .pairing(pairing.clone())
    .serve(my_app_native::APP, my_app_native::methods())
    .bind()
    .await?;

// With port mapping on, let the router answer first, so the ticket carries
// the public address. Off, this returns at once.
let reach = node.reach(Duration::from_secs(5)).await;
// Show this as a QR code, next to the back end, when a device asks to pair.
let ticket = pairing.issue(node.addr(), Duration::from_secs(120));
println!("{ticket}");
```

- **Keep the key, the port and the paired set**, as above, so a device paired
  once still finds the back end, and is still admitted, after a restart.
- **Who may call.** With `pairing`, the node's applications admit the paired
  devices and no one else; the caller's key is proven by the handshake.
  `NodeBuilder::authorizer` replaces that with your own `Authorizer`, which
  also sees the `credential` bytes a front end passes to `openApp`. Without
  either, the node refuses everyone.
- **Unpairing** is `pairing.peers().remove(&id)`, and takes effect on the
  device's next request, even on a connection already open.

`examples/echo/src/bin/kuri-link-echo-server.rs` is a complete server: it
takes the key, port, paired set and relays as arguments, waits for the router
before it issues the ticket and says whether devices outside can reach it,
puts its relays in the ticket, and draws the ticket as a QR code on a
terminal.

### From outside the home

By default a back end opens no port on the home router, so a device away
from home, such as a phone on mobile data, reaches it only through a relay
(see [Relays](#relays)). Forwarding a port adds a direct path, with no relay
and no service in between: turn it on with `NodeBuilder::port_mapping(true)`.
The back end then asks the router itself, by NAT-PMP or UPnP, which home
routers answer in their default settings. It asks for the outside port to be
its own port, and renews the mapping while it runs.

**What turning it on exposes:** the router holds an inbound port that leads to
the back end's machine, for as long as the node runs; after a crash, until the
mapping lapses, at most two hours. Anyone on the internet can send packets to
that port. Only paired devices get past the handshake, but say so to the user
and let them choose. A router that grants only permanent mappings, which
would outlive the back end, is not used.

- **Keep the port.** With `NodeBuilder::port`, the back end listens on the
  same port after a restart, and the router forwards the same outside port
  again, so the address in every ticket it issued stays good. Without it the
  port is random, and every restart gives the back end a new public address,
  which the devices away from home do not learn until they come back. Choose a
  port once, say at random above 1024, and keep it with the key.
- **Ask before issuing a ticket.** `node.reach(timeout)` waits for the
  router's answer, well under a second on a home network, and says what
  devices outside can do:

  | `Reach` | Meaning |
  |---|---|
  | `Public(addr)` | Devices anywhere can dial `addr`. It is in `node.addr()`, so tickets issued from now on carry it. |
  | `BehindNat(addr)` | The router forwards a port, but its own outside address, `addr`, is not public: the ISP puts the home behind its NAT (carrier-grade NAT), or a second router stands in front. |
  | `Unmapped(reason)` | No port is forwarded: mapping is off, or the router does not speak NAT-PMP or UPnP, has them turned off, or grants only permanent mappings. `reason` says which. |

  In the last two cases devices outside cannot reach the back end without a
  relay, while devices on the home network still can. Tell the user so when
  they pair, rather than letting their phone time out later.
- **What still changes the address** is a new public IP from the ISP. The
  back end notices within two minutes, and the devices connected to it learn
  the new address at once (see "Keep the stored address current" under
  [Pair](#5-pair)). A device that is away and not connected at that moment
  learns it once it is back on the home network, if mDNS is on (see below).

Only IPv4 is mapped and judged. The back end's global IPv6 addresses go into
its address as well, but whether the router's firewall lets devices outside
reach them cannot be seen from inside.

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

**Keep the stored address current.** A back end's address can change after
pairing: the ISP gives the home a new IP, or the router gives the back end a
new address at home. A remote client hears of each change from the back end
itself, after its first request and then as it happens, and dials the new
address from then on. Store it, so the next launch dials it too:

```dart
final client = await node.openApp(
  app: 'calc',
  backend: Backend.remote(peer: stored),
  credential: null,
);
final updates = client.backEndUpdates()!;
for (PeerAddr? now; (now = await updates.next()) != null;) {
  save(now!);
}
```

`next()` returns `null` once the client is disposed, which ends the loop.
`client.backEnd()` is the address as the client knows it now. `follow` in
`examples/app/lib/main.dart` does this.

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
directly: a phone on mobile data and a back end whose router forwards no
port, or two devices that are both away from home.
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

A node uses only the relays it started with; a back end behind a gateway also
dials the gateway through the relays in the gateway's address. To reach a back
end through the relays it names, start the node with them:
`ticketBackEnd(ticket:).relays` before pairing, the stored address's `relays`
after. To change relays, shut the node down and start it again from the same
key file; it is still the same device to every back end:

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
Next to a gateway, let the gateway decide whom it carries, so that a relay
meant for one home carries that home alone. Start the gateway with
`--relay-access 127.0.0.1:3341`, and give the relay:

```toml
[access.http]
url = "http://127.0.0.1:3341/"
```

The relay then carries the gateway, and the phones and back ends paired with
it, and no one else. Pairing or unpairing a device needs no change to the
relay's config.

The relay sees only a key, never a ticket, so it does not carry a phone that
is not paired yet. A phone therefore pairs over the home network, where it
reaches the gateway without the relay; a pairing screen should say so.

## Network changes and lost connections

**Android needs a hint when the network changes.** Native code there cannot
watch the network, so without help a switch between WiFi and mobile data
stalls requests until the old path times out, which took 15 to 17 seconds
on a phone. If the connection hears nothing at all for 8 seconds of that, a
request waiting on it fails instead, as below. Watch the default network from
the activity and pass each change to Dart:

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

**At home, the back end can be found by mDNS.** Off by default; turn it on
with `NodeBuilder::local_discovery(true)` on the back end and
`LinkNode.start(..., localDiscovery: true)` on the device. The back end then
announces its key and addresses on the local network, and the device looks
peers up there when it dials. So a device back on the home network finds the
back end even if its address there changed, and learns its new public address
from it. **What it exposes:** everyone on that network, guests and housemates
included, sees the back end's key and addresses, and the device's lookups;
only paired devices get any further. Say so where the user turns it on.

Without it, a device knows only the addresses the back end gave it: at
pairing, and pushed over `kuri-link.addr` while it was connected. When the
back end's address changes while the device is not connected, the device
reaches it again only through a relay, or by pairing again.

Android drops multicast packets before they reach an app that does not hold a
multicast lock, so take one while the app runs, with
`<uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE"/>`
in the manifest:

```kotlin
import android.net.wifi.WifiManager

val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
val multicast = wifi.createMulticastLock("my_app/mdns").apply {
    setReferenceCounted(false)
    acquire()
}
// multicast.release() in cleanUpFlutterEngine.
```

`MainActivity.kt` in `examples/app` does both, when the app asks for it. A
node that serves nothing never announces itself.

iOS lets an app send multicast only with the
`com.apple.developer.networking.multicast` entitlement, which Apple grants on
request. Without it, leave `localDiscovery` off on iOS, as `examples/app`
does.

**A back end only reachable at home is given up on in five seconds.** When
none of a back end's addresses can be reached from outside its own network
and it names no relay, a device elsewhere would wait 30 seconds for an answer
that cannot come. It gives up after five instead, and the `LinkError` says
that only a device on the back end's network can reach it.

**A lost connection is dialed again.** When a remote client's connection is
lost, the client dials again before its next request. A phone out of signal
loses its connection after QUIC's idle timeout, 30 seconds, or after 8
seconds if a request is waiting meanwhile, as below. Everything on the
connection ends with it: every raw stream opened on it, and every request
under way, which fails and is not sent again, since it may already have taken
effect; whether to retry it is the app's decision.

**A back end that crashed is noticed within seconds.** A back end that stops
cleanly closes its connections, and its clients dial again at once. One that
crashes or loses power says nothing, and once it is back on the same address
it does not know the old connections; QUIC's reset cannot tell the clients
either, since the new process makes it with a key of its own. So a request
that hears nothing at all from its back end for 8 seconds, not even an
acknowledgement, takes the connection for lost: it fails, and the next request
dials again. A back end that is only slow is not affected: while it works on
a reply, the two sides' keep-alives pass every five seconds.

Nor is a phone that changes networks, as from WiFi to mobile data: while iroh
looks for a path on the new network, the back end answers its tries on the
paths that work, and a request waits until one of them carries it, as before.
Only if no path at all answers for 8 seconds does the request fail, and the
next one dial again: say because the phone has had no working network for
that long, as when WiFi fades on the way out before the phone moves to mobile
data, or because the back end can be reached neither directly nor through a
relay from the new network. Such a request used to get its reply if the
network came back within the idle timeout. This is what noticing a crashed
back end within seconds costs.

A back end behind a gateway notices from the silence on its own connection,
within 5 to 6 seconds, that the gateway crashed and came back, and registers
again. Until it has, the gateway answers requests for its apps with the fault
`unavailable`. It sends a keep-alive every second and gives up on the
connection after 4 seconds of silence, which suits a back end on the
gateway's own network. One that reaches its gateway through a relay, or over
WiFi that drops out, registers again whenever nothing passes for 3 to 4
seconds, with `unavailable` meanwhile. It dials again a second after each such
loss, however short the registration held. While the gateway cannot be
reached, the waits between tries double, up to 30 seconds; when the back end's
own network changes, as when its WiFi comes back, it dials at once. A gateway
whose own network comes back is found at the next try.

`client.path()` reports the path requests use now, `LinkRoute.direct` or
`LinkRoute.relay`, with its round-trip time; it is `null` in process and while
there is no connection. The **Measure** button in `examples/app` logs it once
a second, which is a quick way to see how an app behaves on a real network.

## Not yet

- **A device away when the home's IP changes.** If the ISP gives the home a
  new public IP while a device is away from home and not connected, the device
  cannot reach the back end until it is back on the home network. Nothing
  inside the home can reach it elsewhere to tell it, and nothing outside the
  home knows.
- **No typed Dart clients.** Dart calls methods by name with CBOR bytes.
- **No Windows builds, and no iOS device has run the app**, as above.
