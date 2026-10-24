// Drives the real app: the same panel, first with the back end in this
// process, then with it in another process, paired by ticket and reached over
// iroh. On Linux the test starts that process itself, on loopback. Build the
// server first, from the repository root:
//
//     cargo build -p kuri-link-echo --bin kuri-link-echo-server
//     cd examples/app && flutter test integration_test -d linux
//
// KURI_LINK_TARGET names another Cargo target directory (default
// ../../target/debug). KURI_LINK_SCREENSHOTS names a directory to save a
// screenshot of each back end's answers in.
//
// A phone or emulator cannot start a process on the host, and the macOS app's
// sandbox lets it start none outside its bundle, so there the server runs
// first and its ticket comes in as a define, or, with KURI_LINK_SCAN, the app
// scans the QR code the server draws; see README.md:
//
//     flutter test integration_test -d <device> --dart-define=KURI_LINK_TICKET=...
//     flutter test integration_test -d <device> --dart-define=KURI_LINK_SCAN=true

import 'dart:convert';
import 'dart:io';
import 'dart:ui' show ImageByteFormat;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show OffsetLayer;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:kuri_link_example/main.dart';

final target = Platform.environment['KURI_LINK_TARGET'] ?? '../../target/debug';
const absentRelay = 'http://127.0.0.1:9/';
const givenTicket = String.fromEnvironment('KURI_LINK_TICKET');
const scan = bool.fromEnvironment('KURI_LINK_SCAN');

/// Pumps frames until `finder` finds something. Calls cross into Rust and
/// back, so real time has to pass, not just fake time.
Future<void> waitFor(
  WidgetTester tester,
  Finder finder, {
  int seconds = 10,
}) async {
  for (var i = 0; i < seconds * 10; i++) {
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

/// Starts an echo server on loopback for this test and returns its ticket.
/// The server names a relay that is not there: the app must still start its
/// node with it, and still reach the server directly.
Future<String> startServer() async {
  final server = await Process.start('$target/kuri-link-echo-server', [
    '--bind',
    '127.0.0.1:0',
    '--relay',
    absentRelay,
  ]);
  server.stderr.drain<void>();
  addTearDown(() async {
    server.kill();
    await server.exitCode;
  });
  return server.stdout
      .transform(utf8.decoder)
      .transform(const LineSplitter())
      .first;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the same panel answers alike in-process and remote', (
    tester,
  ) async {
    final dir = await Directory.systemTemp.createTemp('kuri-link-example-');
    addTearDown(() => dir.delete(recursive: true));
    final ticket = scan || givenTicket.isNotEmpty
        ? givenTicket
        : await startServer();

    final node = await startLink(dir.path);
    final id = node.id();
    await tester.pumpWidget(
      EchoApp(
        node: node,
        store: PeerStore('${dir.path}/back-end.json'),
        dataDir: dir.path,
      ),
    );

    await waitFor(tester, find.text('Back end: in this process'));
    final local = await pressAll(tester);
    await screenshot(tester, 'in-process');

    await tester.tap(find.text('Remote'));
    await waitFor(tester, find.byKey(const Key('ticket')));
    if (scan) {
      // The camera has to start and find the code: give it longer.
      await tester.tap(find.byKey(const Key('scan')));
      await waitFor(
        tester,
        find.text('Back end: remote, over iroh'),
        seconds: 60,
      );
    } else {
      await tester.enterText(find.byKey(const Key('ticket')), ticket);
      await tester.tap(find.widgetWithText(FilledButton, 'Pair'));
      await waitFor(tester, find.text('Back end: remote, over iroh'));
    }
    final remote = await pressAll(tester);
    await screenshot(tester, 'remote');

    // The probe a field test runs: a call a second, logged with its path.
    await tester.tap(find.byKey(const Key('measure')));
    await waitFor(tester, find.textContaining(RegExp(r': [3-9] calls')));
    await tester.tap(find.byKey(const Key('measure')));
    await tester.pump();
    final summary = tester.widget<Text>(find.byKey(const Key('summary'))).data!;
    final logLine = find.textContaining('Log: ');
    final logPath = tester
        .widget<Text>(logLine)
        .data!
        .substring('Log: '.length);
    final log = File(logPath).readAsLinesSync();
    File(logPath).deleteSync();
    debugPrint('probe summary:\n$summary');
    expect(log.first, 'time,ok,rtt_ms,route,to,quic_rtt_ms,error');
    expect(log.length, greaterThan(3));
    expect(summary, contains('failed: 0'));
    if (!scan && givenTicket.isEmpty) {
      // On loopback the path is direct, to the server's own socket.
      expect(summary, startsWith('direct: '));
      expect(log[1].split(',')[3], 'direct');
    }

    expect(local, {
      'echo': 'kuri-link',
      'add': '5',
      'whoami': 'in-process',
      'upper (stream)': 'KURI-LINK',
    });
    // Only the caller differs: remotely, the key the handshake proved.
    expect(remote, {...local, 'whoami': id});
    if (!scan && givenTicket.isEmpty) {
      // Pairing with a back end that names a relay replaced the node it was
      // given with one started with that relay, from the same key file.
      expect(() => node.id(), throwsA(anything));
    }
  });
}
