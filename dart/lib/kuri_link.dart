/// Private RPC between a front end and its back end, which runs in this
/// process or on another device.
///
/// Load the app's native library once with [KuriLinkLib.init], start a
/// [LinkNode], and open the app with [LinkNode.openApp]. Whether the
/// [Backend] is local or remote is configuration: the code after `openApp` is
/// the same.
library;

export 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show ExternalLibrary;

export 'src/rust/api.dart';
export 'src/rust/frb_generated.dart' show KuriLinkLib;
