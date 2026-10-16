//! An example back end, the app `echo`: the same service whether it runs in the
//! front end's process or on another device.
//!
//! Built as a cdylib, this is everything a Flutter app needs on the native
//! side: the Dart bindings (re-exported from `kuri_link_ffi`) and this back end,
//! in one library. Built as the `kuri-link-echo-server` binary, it is the same
//! back end on its own, reached over iroh.

use kuri_link::{AppError, Methods};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Linking the bindings in is what exports their entry points from the cdylib.
pub use kuri_link_ffi;

/// The app name front ends open.
pub const APP: &str = "echo";

/// The service: `echo` returns its argument; `add` sums a pair of integers;
/// `whoami` names the caller's key, or `in-process`; `upper` is a stream that
/// sends back what it reads, upper-cased, once the caller finishes.
pub fn methods() -> Methods {
    Methods::new()
        .unary("echo", |_caller, args| async move { Ok(args) })
        .typed(
            "add",
            |_caller, (a, b): (i64, i64)| async move { Ok(a + b) },
        )
        .typed("whoami", |caller, (): ()| async move {
            Ok(caller
                .peer()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "in-process".into()))
        })
        .duplex("upper", |_caller, _args, mut io| async move {
            let fail = |e: std::io::Error| AppError::Failed(e.to_string());
            let mut data = Vec::new();
            io.recv.read_to_end(&mut data).await.map_err(fail)?;
            io.send
                .write_all(&data.to_ascii_uppercase())
                .await
                .map_err(fail)?;
            io.send.shutdown().await.map_err(fail)?;
            Ok(())
        })
}

/// Registers `echo` as a back end in this process. The front end calls it once
/// at start-up, before opening `Backend.local()`.
#[no_mangle]
pub extern "C" fn kuri_link_echo_register() {
    kuri_link::register_local(APP, methods());
}
