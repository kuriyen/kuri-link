// One app whose back end, `echo`, runs in this process or on another device.
// Which one is a setting; the code that uses the back end is the same. It runs
// on Linux and Android:
//
//     flutter run -d linux                                       # in-process
//     flutter run -d linux --dart-entrypoint-args=--backend=remote
//     flutter run -d <android device>
//
// For a remote back end, start one first, then paste the ticket it prints or,
// on a phone, scan the QR code it draws:
//
//     cargo run -p kuri-link-echo --bin kuri-link-echo-server
//
// A back end started with --relay names its relays in the ticket, and the app
// starts its node with them, so it can reach the back end from another
// network.

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:cbor/cbor.dart';
import 'package:flutter/material.dart';
import 'package:flutter_zxing/flutter_zxing.dart';
import 'package:kuri_link/kuri_link.dart';
import 'package:path_provider/path_provider.dart';

const app = 'echo';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  final dir = await dataDir();
  final store = PeerStore('$dir/back-end.json');
  final node = await startLink(dir, relays: store.load()?.relays ?? const []);
  runApp(
    EchoApp(
      node: node,
      store: store,
      dataDir: dir,
      remote: args.contains('--backend=remote'),
    ),
  );
}

/// Where the device key and the paired back end are kept: the app's own
/// directory, `~/.local/share/io.github.kuriyen.kuri_link_example` on Linux.
Future<String> dataDir() async => (await getApplicationSupportDirectory()).path;

/// The device key: the same device to every back end, whichever relays the
/// node uses.
String keyPath(String dataDir) => '$dataDir/device.key';

/// The app's one native library. On Linux it is bundled next to the
/// executable; on Android it is in the APK, where the loader finds it by name.
String nativeLibrary() => Platform.isAndroid
    ? 'libkuri_link_echo.so'
    : '${File(Platform.resolvedExecutable).parent.path}/lib/libkuri_link_echo.so';

/// Loads the app's one native library, which holds both the Kuri Link bindings
/// and the echo back end, registers that back end, and starts this device's
/// node with `relays`: those of the paired back end, if any.
Future<LinkNode> startLink(
  String dataDir, {
  List<String> relays = const [],
}) async {
  final library = nativeLibrary();
  await KuriLinkLib.init(externalLibrary: ExternalLibrary.open(library));
  DynamicLibrary.open(library).lookupFunction<Void Function(), void Function()>(
    'kuri_link_echo_register',
  )();
  await Directory(dataDir).create(recursive: true);
  return LinkNode.start(keyPath: keyPath(dataDir), relays: relays);
}

/// The paired back end's address, kept across launches.
class PeerStore {
  PeerStore(this.path);

  final String path;

  PeerAddr? load() {
    final file = File(path);
    if (!file.existsSync()) return null;
    final json = jsonDecode(file.readAsStringSync()) as Map<String, dynamic>;
    return PeerAddr(
      id: json['id'] as String,
      addrs: (json['addrs'] as List).cast<String>(),
      relays: (json['relays'] as List).cast<String>(),
    );
  }

  void save(PeerAddr? peer) {
    final file = File(path);
    if (peer == null) {
      if (file.existsSync()) file.deleteSync();
      return;
    }
    file.writeAsStringSync(
      jsonEncode({'id': peer.id, 'addrs': peer.addrs, 'relays': peer.relays}),
    );
  }
}

class EchoApp extends StatefulWidget {
  const EchoApp({
    super.key,
    required this.node,
    required this.store,
    required this.dataDir,
    this.remote = false,
  });

  final LinkNode node;
  final PeerStore store;
  final String dataDir;
  final bool remote;

  @override
  State<EchoApp> createState() => _EchoAppState();
}

class _EchoAppState extends State<EchoApp> {
  late LinkNode node = widget.node;
  late bool remote = widget.remote;
  late PeerAddr? peer = widget.store.load();
  Future<LinkClient>? client;
  String? pairError;
  final ticket = TextEditingController();

  @override
  void initState() {
    super.initState();
    open();
  }

  /// The one place the two back ends differ.
  void open() {
    final Backend? backend = !remote
        ? const Backend.local()
        : peer == null
        ? null
        : Backend.remote(peer: peer!);
    client = backend == null
        ? null
        : node.openApp(app: app, backend: backend, credential: null);
  }

  /// Fills in the ticket from the back end's QR code, then pairs.
  Future<void> scan(BuildContext context) async {
    final scanned = await Navigator.of(
      context,
    ).push(MaterialPageRoute<String>(builder: (_) => const ScanPage()));
    if (scanned == null) return;
    ticket.text = scanned;
    await pair();
  }

  Future<void> pair() async {
    try {
      final text = ticket.text.trim();
      await useRelaysOf(ticketBackEnd(ticket: text));
      final paired = await node.pair(ticket: text);
      widget.store.save(paired);
      setState(() {
        peer = paired;
        pairError = null;
        ticket.clear();
        open();
      });
    } on LinkError catch (e) {
      setState(() => pairError = e.toString());
    }
  }

  /// A node uses only the relays it started with, so a back end that names
  /// others gets a new node. The key is the same, so it is the same device.
  Future<void> useRelaysOf(PeerAddr backEnd) async {
    final have = node.relays().toSet();
    final want = backEnd.relays.toSet();
    if (have.length == want.length && have.containsAll(want)) return;
    final old = node;
    node = await LinkNode.start(
      keyPath: keyPath(widget.dataDir),
      relays: backEnd.relays,
    );
    await old.shutdown();
  }

  void forget() {
    widget.store.save(null);
    setState(() {
      peer = null;
      open();
    });
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Kuri Link: echo',
      theme: ThemeData(colorSchemeSeed: Colors.teal),
      darkTheme: ThemeData(
        colorSchemeSeed: Colors.teal,
        brightness: Brightness.dark,
      ),
      home: Scaffold(
        appBar: AppBar(title: const Text('Kuri Link: echo')),
        body: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SegmentedButton<bool>(
                segments: const [
                  ButtonSegment(
                    value: false,
                    label: Text('In this process'),
                    icon: Icon(Icons.memory),
                  ),
                  ButtonSegment(
                    value: true,
                    label: Text('Remote'),
                    icon: Icon(Icons.lan),
                  ),
                ],
                selected: {remote},
                onSelectionChanged: (s) => setState(() {
                  remote = s.single;
                  open();
                }),
              ),
              const SizedBox(height: 12),
              if (remote) pairing(),
              const SizedBox(height: 12),
              Expanded(
                child: client == null
                    ? const SizedBox()
                    : FutureBuilder<LinkClient>(
                        key: ObjectKey(client),
                        future: client,
                        builder: (context, snapshot) {
                          if (snapshot.hasError) {
                            return Text('${snapshot.error}');
                          }
                          if (!snapshot.hasData) {
                            return const Center(
                              child: CircularProgressIndicator(),
                            );
                          }
                          return EchoPanel(client: snapshot.data!);
                        },
                      ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget pairing() {
    final paired = peer;
    if (paired != null) {
      return Row(
        children: [
          Expanded(
            child: Text(
              'Paired with ${paired.id.substring(0, 10)}, '
              'at ${[...paired.addrs, ...paired.relays].join(', ')}',
            ),
          ),
          TextButton(onPressed: forget, child: const Text('Forget')),
        ],
      );
    }
    return Row(
      children: [
        Expanded(
          child: TextField(
            key: const Key('ticket'),
            controller: ticket,
            decoration: InputDecoration(
              labelText: 'Ticket from the back end (KURI-LINK:...)',
              errorText: pairError,
            ),
            onSubmitted: (_) => pair(),
          ),
        ),
        const SizedBox(width: 8),
        // Phones have a camera to read the back end's QR code with.
        if (Platform.isAndroid || Platform.isIOS)
          Builder(
            builder: (context) => IconButton(
              key: const Key('scan'),
              tooltip: 'Scan the QR code',
              icon: const Icon(Icons.qr_code_scanner),
              onPressed: () => scan(context),
            ),
          ),
        FilledButton(onPressed: pair, child: const Text('Pair')),
      ],
    );
  }
}

/// The camera, until it sees a Kuri Link ticket's QR code; then returns the
/// ticket's text. Other QR codes are ignored.
///
/// The decoder is zxing-cpp, on the device, with nothing sent anywhere. The
/// common alternative on Android, ML Kit, reports usage to Google, and this
/// example is meant to show an app that talks to its own back end only.
class ScanPage extends StatefulWidget {
  const ScanPage({super.key});

  @override
  State<ScanPage> createState() => _ScanPageState();
}

class _ScanPageState extends State<ScanPage> {
  bool done = false;

  void scanned(Code code) {
    final text = code.text;
    if (done || !code.isValid || text == null) return;
    if (!text.toUpperCase().startsWith('KURI-LINK:')) return;
    done = true;
    Navigator.of(context).pop(text);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Scan the back end\'s QR code')),
      body: ReaderWidget(
        codeFormat: Format.qrCode,
        onScan: scanned,
        // The whole frame, so the code need not be lined up in a box.
        cropPercent: 1,
        showScannerOverlay: false,
        showGallery: false,
        showToggleCamera: false,
      ),
    );
  }
}

/// Everything that talks to the back end. It gets a [LinkClient] and never
/// learns where the back end runs, beyond [LinkClient.isLocal] for the label.
class EchoPanel extends StatefulWidget {
  const EchoPanel({super.key, required this.client});

  final LinkClient client;

  @override
  State<EchoPanel> createState() => _EchoPanelState();
}

class _EchoPanelState extends State<EchoPanel> {
  final text = TextEditingController(text: 'kuri-link');
  final a = TextEditingController(text: '2');
  final b = TextEditingController(text: '3');
  final results = <String>[];

  LinkClient get client => widget.client;

  static Uint8List encode(CborValue value) =>
      Uint8List.fromList(cbor.encode(value));

  Future<void> run(String label, Future<String> Function() request) async {
    String line;
    try {
      line = '$label → ${await request()}';
    } on LinkError catch (e) {
      line = '$label ✗ $e';
    }
    if (mounted) setState(() => results.insert(0, line));
  }

  Future<String> echo() async {
    final out = await client.call(
      method: 'echo',
      argsCbor: encode(CborString(text.text)),
    );
    return cbor.decode(out).toString();
  }

  Future<String> add() async {
    final args = CborList([
      CborSmallInt(int.parse(a.text)),
      CborSmallInt(int.parse(b.text)),
    ]);
    final out = await client.call(method: 'add', argsCbor: encode(args));
    return (cbor.decode(out) as CborInt).toInt().toString();
  }

  Future<String> whoami() async {
    final out = await client.call(method: 'whoami', argsCbor: Uint8List(0));
    return cbor.decode(out).toString();
  }

  Future<String> upper() async {
    final io = await client.open(method: 'upper', argsCbor: Uint8List(0));
    await io.write(data: utf8.encode(text.text));
    await io.finish();
    return utf8.decode(await io.readToEnd());
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          client.isLocal()
              ? 'Back end: in this process'
              : 'Back end: remote, over iroh',
          key: const Key('where'),
          style: Theme.of(context).textTheme.titleMedium,
        ),
        const SizedBox(height: 8),
        TextField(
          key: const Key('text'),
          controller: text,
          decoration: const InputDecoration(labelText: 'Text'),
        ),
        Row(
          children: [
            SizedBox(
              width: 80,
              child: TextField(
                key: const Key('a'),
                controller: a,
                decoration: const InputDecoration(labelText: 'a'),
              ),
            ),
            const SizedBox(width: 8),
            SizedBox(
              width: 80,
              child: TextField(
                key: const Key('b'),
                controller: b,
                decoration: const InputDecoration(labelText: 'b'),
              ),
            ),
          ],
        ),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            OutlinedButton(
              onPressed: () => run('echo', echo),
              child: const Text('echo'),
            ),
            OutlinedButton(
              onPressed: () => run('add', add),
              child: const Text('add'),
            ),
            OutlinedButton(
              onPressed: () => run('whoami', whoami),
              child: const Text('whoami'),
            ),
            OutlinedButton(
              onPressed: () => run('upper (stream)', upper),
              child: const Text('upper (stream)'),
            ),
          ],
        ),
        const SizedBox(height: 12),
        Expanded(
          child: ListView(
            key: const Key('results'),
            children: [for (final line in results) Text(line)],
          ),
        ),
      ],
    );
  }
}
