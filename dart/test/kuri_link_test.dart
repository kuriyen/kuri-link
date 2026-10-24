// End to end against the example back end `echo` (examples/echo): once linked
// into this process, once as its own process reached over iroh on loopback.
// The same assertions run against both, as in the Rust loopback tests.
//
// Build first, from the repository root:
//
//     cargo build -p kuri-link-echo
//
// KURI_LINK_TARGET names another Cargo target directory (default ../target/debug).

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:cbor/cbor.dart';
import 'package:kuri_link/kuri_link.dart';
import 'package:test/test.dart';

final target = Platform.environment['KURI_LINK_TARGET'] ?? '../target/debug';
final library = Platform.isMacOS
    ? '$target/libkuri_link_echo.dylib'
    : '$target/libkuri_link_echo.so';
final serverBinary = '$target/kuri-link-echo-server';

const app = 'echo';

Uint8List encode(CborValue value) => Uint8List.fromList(cbor.encode(value));
CborValue decode(List<int> bytes) => cbor.decode(bytes);
final none = Uint8List(0);

Matcher faultOf(FaultCode? code) =>
    throwsA(isA<LinkError>().having((e) => e.fault, 'fault', code));

/// The same assertions, run against a back end over the network and one in
/// this process: a front end must not be able to tell them apart.
Future<void> exercise(LinkClient client, String expectedCaller) async {
  final hi = CborString('hi');
  expect(decode(await client.call(method: 'echo', argsCbor: encode(hi))), hi);

  final pair = encode(CborList([CborSmallInt(2), CborSmallInt(3)]));
  final sum = decode(await client.call(method: 'add', argsCbor: pair));
  expect((sum as CborInt).toInt(), 5);

  // Over the network the server sees the key the handshake proved, not
  // anything the client said; in-process there is no key at all.
  final seen = decode(await client.call(method: 'whoami', argsCbor: none));
  expect((seen as CborString).toString(), expectedCaller);

  final io = await client.open(method: 'upper', argsCbor: none);
  await io.write(data: utf8.encode('kuri-link'));
  await io.finish();
  expect(utf8.decode(await io.readToEnd()), 'KURI-LINK');

  // Reading and writing do not wait for each other.
  final both = await client.open(method: 'upper', argsCbor: none);
  final reading = both.readToEnd();
  await both.write(data: utf8.encode('both ways'));
  await both.finish();
  expect(utf8.decode(await reading), 'BOTH WAYS');

  await expectLater(
    client.call(method: 'nope', argsCbor: none),
    faultOf(FaultCode.methodNotFound),
  );
  await expectLater(
    client.open(method: 'echo', argsCbor: none),
    faultOf(FaultCode.methodNotFound),
    reason: 'echo is not an open method',
  );
  await expectLater(
    client.call(method: 'add', argsCbor: encode(CborString('not a pair'))),
    faultOf(FaultCode.badRequest),
  );

  // Requests run concurrently, each on its own stream.
  final sums = await Future.wait([
    for (var i = 0; i < 16; i++)
      client
          .call(
            method: 'add',
            argsCbor: encode(CborList([CborSmallInt(i), CborSmallInt(i)])),
          )
          .then((r) => (decode(r) as CborInt).toInt()),
  ]);
  expect(sums, [for (var i = 0; i < 16; i++) 2 * i]);
}

void main() {
  late Directory dir;
  late Process server;
  late String ticket;
  late LinkNode node;
  late PeerAddr backEnd;

  setUpAll(() async {
    await KuriLinkLib.init(externalLibrary: ExternalLibrary.open(library));
    DynamicLibrary.open(
      library,
    ).lookupFunction<Void Function(), void Function()>(
      'kuri_link_echo_register',
    )();

    dir = await Directory.systemTemp.createTemp('kuri-link-test-');
    server = await Process.start(serverBinary, ['--bind', '127.0.0.1:0']);
    server.stderr.drain<void>();
    ticket = await server.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .first;

    node = await LinkNode.start(keyPath: '${dir.path}/device.key', relays: []);
    backEnd = await node.pair(ticket: ticket);
  });

  tearDownAll(() async {
    server.kill();
    await server.exitCode;
    await dir.delete(recursive: true);
  });

  // The front end's code below `openApp` is the same for both.
  final backends = {
    'local': () => const Backend.local(),
    'remote': () => Backend.remote(peer: backEnd),
  };
  for (final MapEntry(key: name, value: backend) in backends.entries) {
    test('a $name back end answers', () async {
      final client = await node.openApp(
        app: app,
        backend: backend(),
        credential: null,
      );
      expect(client.isLocal(), name == 'local');
      await exercise(client, name == 'local' ? 'in-process' : node.id());
      // Remotely, with relays off, the path is the server's own socket.
      final path = client.path();
      if (name == 'local') {
        expect(path, isNull);
        expect(client.backEnd(), isNull);
        expect(client.backEndUpdates(), isNull);
      } else {
        expect(path!.route, LinkRoute.direct(addr: backEnd.addrs.single));
        expect(path.rttMicros, greaterThan(0));
        // The server lists the same socket it put in the ticket.
        expect(client.backEnd()!.id, backEnd.id);
        expect(client.backEnd()!.addrs, backEnd.addrs);
        final updates = client.backEndUpdates()!;
        client.dispose();
        expect(await updates.next(), isNull, reason: 'the client is gone');
      }
    });
  }

  test('a ticket names its back end before it is redeemed', () async {
    final named = ticketBackEnd(ticket: ticket);
    expect(named.id, backEnd.id);
    expect(named.addrs, backEnd.addrs);
    expect(named.relays, isEmpty);
    expect(
      () => ticketBackEnd(ticket: 'KURI-LINK:nonsense'),
      throwsA(isA<LinkError>()),
    );

    final relayed = await Process.start(serverBinary, [
      '--bind',
      '127.0.0.1:0',
      '--relay',
      'http://127.0.0.1:9',
    ]);
    relayed.stderr.drain<void>();
    final relayedTicket = await relayed.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .first;
    relayed.kill();
    await relayed.exitCode;
    expect(ticketBackEnd(ticket: relayedTicket).relays, [
      'http://127.0.0.1:9/',
    ]);
  });

  test(
    'a node shut down and started again with other relays is the same device',
    () async {
      final keyPath = '${dir.path}/restart.key';
      final first = await LinkNode.start(
        keyPath: keyPath,
        relays: ['http://127.0.0.1:9'],
      );
      expect(first.relays(), ['http://127.0.0.1:9/']);
      final id = first.id();
      await first.shutdown();

      final second = await LinkNode.start(keyPath: keyPath, relays: []);
      expect(second.id(), id);
      expect(second.relays(), isEmpty);
      await second.shutdown();
    },
  );

  test('a ticket pairs one device', () async {
    final other = await LinkNode.start(
      keyPath: '${dir.path}/other.key',
      relays: [],
    );
    await expectLater(other.pair(ticket: ticket), faultOf(FaultCode.denied));
  });

  test('an unpaired device is denied on every request', () async {
    final stranger = await LinkNode.start(
      keyPath: '${dir.path}/stranger.key',
      relays: [],
    );
    final client = await stranger.openApp(
      app: app,
      backend: Backend.remote(peer: backEnd),
      credential: null,
    );
    await expectLater(
      client.call(method: 'echo', argsCbor: none),
      faultOf(FaultCode.denied),
    );
    await expectLater(
      client.open(method: 'upper', argsCbor: none),
      faultOf(FaultCode.denied),
    );
  });

  test('the key file keeps the device the same', () async {
    final again = await LinkNode.start(
      keyPath: '${dir.path}/device.key',
      relays: [],
    );
    expect(again.id(), node.id());
  });

  test('errors that never reach a back end carry no fault', () async {
    await expectLater(
      node.openApp(
        app: 'not-registered',
        backend: const Backend.local(),
        credential: null,
      ),
      faultOf(null),
    );
    await expectLater(node.pair(ticket: 'KURI-LINK:'), faultOf(null));
    await expectLater(
      node.openApp(
        app: app,
        backend: Backend.remote(
          peer: PeerAddr(id: 'not a key', addrs: const [], relays: const []),
        ),
        credential: null,
      ),
      faultOf(null),
    );
  });
}
