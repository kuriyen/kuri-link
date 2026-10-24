# Example app

A Flutter app for Linux, Android, macOS and iOS whose back end, `echo` from
`examples/echo`, runs in the app's own process or on another device. The choice
is a switch at the top of the window; `EchoPanel` in `lib/main.dart`, the code
that calls the back end, is the same either way.

The app ships one native library, `libkuri_link_echo.so`: the Kuri Link
bindings and the `echo` back end together. See `dart/README.md` for why it must
be one library. Each platform's build runs Cargo for it:
`linux/CMakeLists.txt` on Linux, `android/app/build.gradle.kts` on Android,
and `darwin/build_library.sh`, a build phase of the Xcode project, on macOS
and iOS.

## Run on Linux

```sh
flutter run -d linux                     # back end in this process
```

For a remote back end, start one, then choose **Remote** and paste the ticket
it prints. The app keeps the paired address, so later launches reconnect
without a ticket as long as the server keeps its key and its paired devices:

```sh
cargo run -p kuri-link-echo --bin kuri-link-echo-server -- \
    --bind 127.0.0.1:4433 --key server.key --peers paired-devices
flutter run -d linux --dart-entrypoint-args=--backend=remote
```

## Run on Android

Cargo needs the Android targets and [cargo-ndk](https://github.com/bbqsrc/cargo-ndk),
which points it at the NDK's linker; Gradle finds the NDK itself:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
flutter run -d <device>
```

The APK carries the library for arm64-v8a (phones) and x86_64 (the emulator).
For a remote back end, run the server where the phone can reach it, without
`--bind`, so its ticket lists the host's addresses; the emulator reaches its
host's addresses through its own network.

Android does not let native code watch the network, so `MainActivity` does:
when the default network changes, say from WiFi to mobile data, it tells the
Dart side, which calls `node.networkChanged()`. Without that, requests stall
for about as long as the old path takes to time out, some 15 seconds.

## Run on macOS

```sh
flutter run -d macos
```

Xcode runs Cargo in a build phase, from its own environment, so Cargo has to
be on Xcode's PATH or in `~/.cargo/bin`. The build puts the library in the
app's `Contents/Frameworks` and signs it with the app's identity.

The app is sandboxed, with the entitlements to open connections and to accept
them (`macos/Runner/*.entitlements`). The sandbox lets it start no program
outside its bundle, so its integration test cannot start a server itself:
start one first and pass its ticket, as for a phone below.

```sh
cargo run -p kuri-link-echo --bin kuri-link-echo-server -- --bind 127.0.0.1:0
flutter test integration_test -d macos --dart-define=KURI_LINK_TICKET=KURI-LINK:...
```

## Run on iOS

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
flutter run -d <device or simulator>
```

A build for the simulator in general, as `flutter build ios --simulator`
makes, is for both of the simulator's architectures, hence the x86_64 target.

iOS loads code only from frameworks, so there the same build phase makes the
library the binary of `kuri_link_echo.framework`, in the app's `Frameworks`.
A device needs a development team to sign the app with, chosen in Xcode
(`ios/Runner.xcworkspace`).

`Info.plist` says why the app uses the camera, for the scan button, and the
local network. iOS lets an app send multicast only with an entitlement Apple
grants on request, so on iOS the app does not look its back end up by mDNS; at
home it reaches the back end at the address it kept.

## Pair by QR code

On a terminal, the server draws its ticket as a QR code below the line of
text. On a phone, choose **Remote**, press the scan button next to **Pair**, and
point the camera at the code. The app takes the first Kuri Link ticket it sees
and ignores any other QR code. The Linux app has no scan button; paste the
ticket there.

The code is decoded on the phone by zxing-cpp (the `flutter_zxing` package),
and nothing is sent anywhere else. ML Kit, the usual decoder on Android,
reports usage to Google, and this app is meant to talk to its own back end
only. For the same reason the app asks only for the network and the camera,
not for the microphone or storage that its camera and gallery plugins would
otherwise request.

## Through a relay

When the phone and the server are on different networks, such as the phone on
mobile data, there may be no direct path between them. Run a relay where both
can reach it, such as [`iroh-relay`](https://crates.io/crates/iroh-relay) on a
host with a public address, and name it when starting the server:

```sh
cargo run -p kuri-link-echo --bin kuri-link-echo-server -- \
    --relay https://relay.example.org --key server.key --peers paired-devices
```

The ticket names the relay, and the app starts its node with it before
pairing; on later launches it uses the relays of the address it kept. The relay
forwards encrypted packets and sees which keys talk to each other, never what
they say.

## Measure

With a remote back end, **Measure** makes one `echo` call a second until
**Stop**, and shows, per path (direct or through a relay), how many calls went
that way and their median and 95th-percentile round-trip times. Each call is
logged as a line of CSV: the time, whether it succeeded, the round-trip time,
the path, and QUIC's own estimate of the path's round-trip time. A call that
waits while the network changes under it holds up the next ones, so the log
shows the gap.

The log is in the app's external files directory on Android, which `adb` can
read without root, and in its support directory elsewhere:

```sh
adb pull /sdcard/Android/data/io.github.kuriyen.kuri_link_example/files/ .
```

## Where the app keeps things

The device key and the paired address live in the app's support directory:
`~/.local/share/io.github.kuriyen.kuri_link_example` on Linux, the sandbox's
`~/Library/Containers/io.github.kuriyen.kuriLinkExample/Data/Library/Application Support/io.github.kuriyen.kuriLinkExample`
on macOS, and the app's private storage on Android and iOS.

## Test

On Linux the test starts its own server, on loopback:

```sh
cargo build -p kuri-link-echo --bin kuri-link-echo-server
flutter test integration_test -d linux
```

It drives the app: presses every button with the back end in-process, pairs
with the server, presses them again, and expects the same answers apart from
`whoami`.

A phone, the Android emulator or the iOS simulator cannot start a process on
the host, so start the server first and hand the test its ticket:

```sh
cargo run -p kuri-link-echo --bin kuri-link-echo-server > ticket &
until [ -s ticket ]; do sleep 1; done
flutter test integration_test -d <device> --dart-define=KURI_LINK_TICKET=$(head -1 ticket)
```

On the emulator this pairs over the emulator's network with the server on its
host; a stopped server makes the test fail. The iOS simulator shares the Mac's
network, so there the server can stay on loopback, as for macOS above.

To test the path through a relay, bind the server to the host's loopback,
which the emulator cannot reach, and run a relay on an address it can, here
`iroh-relay --dev` on port 3340 of the host's LAN address:

```sh
iroh-relay --dev &
cargo run -p kuri-link-echo --bin kuri-link-echo-server -- \
    --bind 127.0.0.1:0 --relay http://192.0.2.10:3340 > ticket &
```

Then run the test with the ticket as above. Without `--relay`, the test fails.

With `KURI_LINK_SCAN=true` the test pairs by pressing the scan button instead,
so the camera has to find the server's QR code. On the emulator, start it with
`-camera-back virtualscene`, put a picture of the code on the virtual room's
wall, walk the camera to it, and grant the camera permission as soon as the
test installs the app:

```sh
qrencode -s 10 -o ticket.png "$(head -1 ticket)"
adb emu virtualscene-image wall "$PWD/ticket.png"
adb emu automation play "$ANDROID_HOME/emulator/resources/macros/Walk_to_image_room"
until adb shell pm grant io.github.kuriyen.kuri_link_example android.permission.CAMERA 2>/dev/null; do sleep 1; done &
flutter test integration_test -d <emulator> --dart-define=KURI_LINK_SCAN=true
```

With the default picture on the wall instead, the test fails.
