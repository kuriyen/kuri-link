// Drives the real app: the same panel, first with the back end in this
// process, then with it in another process, paired by ticket and reached over
// iroh on loopback. Build the server first, from the repository root:
//
//     cargo build -p kuri-link-echo --bin kuri-link-echo-server
//     cd examples/desktop && flutter test integration_test -d linux
//
// KURI_LINK_TARGET names another Cargo target directory (default
// ../../target/debug). KURI_LINK_SCREENSHOTS names a directory to save a
// screenshot of each back end's answers in.

import 'dart:convert';
import 'dart:io';
import 'dart:ui' show ImageByteFormat;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show OffsetLayer;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:kuri_link_desktop/main.dart';

final target = Platform.environment['KURI_LINK_TARGET'] ?? '../../target/debug';

/// Pumps frames until `finder` finds something. Calls cross into Rust and
/// back, so real time has to pass, not just fake time.
Future<void> waitFor(WidgetTester tester, Finder finder) async {
  for (var i = 0; i < 100; i++) {
    await tester.pump();
    if (finder.evaluate().isNotEmpty) return;
    await Future<void>.delayed(const Duration(milliseconds: 100));
  }
  throw TestFailure('timed out waiting for $finder');
}

final screenshots = Platform.environment['KURI_LINK_SCREENSHOTS'];

Future<void> screenshot(WidgetTester tester, String name) async {
  if (screenshots == null) return;
  final view = tester.binding.renderViews.first;
  final image = await (view.debugLayer! as OffsetLayer).toImage(
    Offset.zero & view.size,
    pixelRatio: tester.view.devicePixelRatio,
  );
  final png = await image.toByteData(format: ImageByteFormat.png);
  await File('$screenshots/$name.png').writeAsBytes(png!.buffer.asUint8List());
}

/// Presses every button on the panel and returns what each answered.
Future<Map<String, String>> pressAll(WidgetTester tester) async {
  final answers = <String, String>{};
  for (final label in ['echo', 'add', 'whoami', 'upper (stream)']) {
    await tester.tap(find.widgetWithText(OutlinedButton, label));
    final line = find.textContaining('$label → ');
    await waitFor(tester, line);
    final text = tester.widget<Text>(line).data!;
    answers[label] = text.substring('$label → '.length);
  }
  return answers;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the same panel answers alike in-process and remote', (
    tester,
  ) async {
    final dir = await Directory.systemTemp.createTemp('kuri-link-desktop-');
    final server = await Process.start('$target/kuri-link-echo-server', [
      '--bind',
      '127.0.0.1:0',
    ]);
    server.stderr.drain<void>();
    final ticket = await server.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .first;
    addTearDown(() async {
      server.kill();
      await server.exitCode;
      await dir.delete(recursive: true);
    });

    final node = await startLink(dir.path);
    await tester.pumpWidget(
      EchoApp(node: node, store: PeerStore('${dir.path}/back-end.json')),
    );

    await waitFor(tester, find.text('Back end: in this process'));
    final local = await pressAll(tester);
    await screenshot(tester, 'in-process');

    await tester.tap(find.text('Remote'));
    await waitFor(tester, find.byKey(const Key('ticket')));
    await tester.enterText(find.byKey(const Key('ticket')), ticket);
    await tester.tap(find.widgetWithText(FilledButton, 'Pair'));
    await waitFor(tester, find.text('Back end: remote, over iroh'));
    final remote = await pressAll(tester);
    await screenshot(tester, 'remote');

    expect(local, {
      'echo': 'kuri-link',
      'add': '5',
      'whoami': 'in-process',
      'upper (stream)': 'KURI-LINK',
    });
    // Only the caller differs: remotely, the key the handshake proved.
    expect(remote, {...local, 'whoami': node.id()});
  });
}
