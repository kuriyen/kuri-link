# Example app

A Flutter app for Linux and Android whose back end, `echo` from
`examples/echo`, runs in the app's own process or on another device. The choice
is a switch at the top of the window; `EchoPanel` in `lib/main.dart`, the code
that calls the back end, is the same either way.

The app ships one native library, `libkuri_link_echo.so`: the Kuri Link
bindings and the `echo` back end together. See `dart/README.md` for why it must
be one library. Each platform's build runs Cargo for it:
`linux/CMakeLists.txt` on Linux, `android/app/build.gradle.kts` on Android.

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

## Where the app keeps things

The device key and the paired address live in the app's support directory:
`~/.local/share/io.github.kuriyen.kuri_link_example` on Linux, and the app's
private storage on Android.

## Test

On Linux the test starts its own server, on loopback:

```sh
cargo build -p kuri-link-echo --bin kuri-link-echo-server
flutter test integration_test -d linux
```

It drives the app: presses every button with the back end in-process, pairs
with the server, presses them again, and expects the same answers apart from
`whoami`.

A phone or the emulator cannot start a process on the host, so start the
server first and hand the test its ticket:

```sh
cargo run -p kuri-link-echo --bin kuri-link-echo-server > ticket &
until [ -s ticket ]; do sleep 1; done
flutter test integration_test -d <device> --dart-define=KURI_LINK_TICKET=$(head -1 ticket)
```

On the emulator this pairs over the emulator's network with the server on its
host; a stopped server makes the test fail.
