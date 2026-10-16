//! The surface a Dart front end binds to.
//!
//! Every type here is plain — strings, byte vectors, simple structs — so that
//! `flutter_rust_bridge_codegen` can generate the Dart side without hand-written
//! glue. Arguments and results cross as CBOR bytes: the Dart side encodes them
//! (the `cbor` package), which keeps one path for both compiled front ends and
//! front ends that only hold untyped maps (a UI described as data).
//!
//! An app ships one native library, built from its own crate: that crate
//! depends on this one and re-exports it (`pub use kuri_link_ffi;`), so the
//! generated entry points are exported, and links in its local back ends.
//! Two libraries would mean two registries, and `Backend::Local` would find
//! nothing.

mod api;
mod frb_generated;

pub use api::*;
