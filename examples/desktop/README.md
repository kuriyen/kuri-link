# Desktop example

A Flutter app for Linux whose back end, `echo` from `examples/echo`, runs in
the app's own process or in another one. The choice is a switch at the top of
the window; `EchoPanel` in `lib/main.dart`, the code that calls the back end,
is the same either way.

The app ships one native library, `libkuri_link_echo.so`: the Kuri Link
bindings and the `echo` back end together. `linux/CMakeLists.txt` builds it
with Cargo and bundles it. See `dart/README.md` for why it must be one library.

## Run

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

The device key and the paired address live in
`$XDG_DATA_HOME/kuri-link-desktop` (default `~/.local/share/kuri-link-desktop`).

## Test

```sh
cargo build -p kuri-link-echo --bin kuri-link-echo-server
flutter test integration_test -d linux
```

It drives the app: presses every button with the back end in-process, pairs
with a server started for the test, presses them again, and expects the same
answers apart from `whoami`.
