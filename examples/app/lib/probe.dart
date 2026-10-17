// Measures the link for a field test: one `echo` a second, each with its
// round-trip time and the path it took, kept as CSV. On Android the file is in
// the app's external files directory, where `adb pull` reaches it.

import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:kuri_link/kuri_link.dart';
import 'package:path_provider/path_provider.dart';

/// One answered call: which way it went and how long it took.
typedef Sample = ({String route, Duration rtt});

/// The `p`th percentile (0 to 100) of `sorted`, by nearest rank.
Duration percentile(List<Duration> sorted, int p) {
  final rank = (p / 100 * sorted.length).ceil().clamp(1, sorted.length);
  return sorted[rank - 1];
}

/// Per route: how many calls went that way, and their median and 95th
/// percentile round-trip times. Calls that failed are counted apart.
String summarize(List<Sample> samples, int failures) {
  final lines = <String>[];
  for (final route in ['direct', 'relay', 'none']) {
    final rtts = [
      for (final s in samples)
        if (s.route == route) s.rtt,
    ]..sort();
    if (rtts.isEmpty) continue;
    final share = (100 * rtts.length / samples.length).round();
    lines.add(
      '$route: ${rtts.length} calls ($share%), '
      'p50 ${percentile(rtts, 50).inMilliseconds} ms, '
      'p95 ${percentile(rtts, 95).inMilliseconds} ms',
    );
  }
  lines.add('failed: $failures');
  return lines.join('\n');
}

/// Where the route of `path` goes, as two CSV fields.
(String, String) describe(LinkPath? path) => switch (path?.route) {
  LinkRoute_Direct(:final addr) => ('direct', addr),
  LinkRoute_Relay(:final url) => ('relay', url),
  null => ('none', ''),
};

String csvField(String s) =>
    s.contains(RegExp('[,"\n]')) ? '"${s.replaceAll('"', '""')}"' : s;

class Probe extends StatefulWidget {
  const Probe({super.key, required this.client});

  final LinkClient client;

  @override
  State<Probe> createState() => _ProbeState();
}

class _ProbeState extends State<Probe> {
  Timer? timer;
  IOSink? log;
  String? logPath;
  final samples = <Sample>[];
  int failures = 0;
  String current = '';
  bool waiting = false;

  Future<void> start() async {
    final dir = Platform.isAndroid
        ? (await getExternalStorageDirectory())!
        : await getApplicationSupportDirectory();
    final stamp = DateTime.now().toUtc().toIso8601String().split('.').first;
    final path = '${dir.path}/measure-${stamp.replaceAll(':', '-')}.csv';
    log = File(path).openWrite()
      ..writeln('time,ok,rtt_ms,route,to,quic_rtt_ms,error');
    setState(() {
      logPath = path;
      samples.clear();
      failures = 0;
    });
    timer = Timer.periodic(const Duration(seconds: 1), (_) => tick());
  }

  Future<void> stop() async {
    timer?.cancel();
    timer = null;
    await log?.close();
    log = null;
    if (mounted) setState(() {});
  }

  /// One call. While a call is still waiting, which happens when the network
  /// changes under it, later ticks are skipped: the log then shows the gap,
  /// and the call's round-trip time says how long it took to get through.
  Future<void> tick() async {
    if (waiting) return;
    waiting = true;
    final sent = DateTime.now();
    String? error;
    try {
      await widget.client.call(method: 'echo', argsCbor: Uint8List(0));
    } on LinkError catch (e) {
      error = e.toString();
    }
    final rtt = DateTime.now().difference(sent);
    waiting = false;
    final path = widget.client.path();
    final (route, to) = describe(path);
    final quicRtt = path == null
        ? ''
        : (path.rttMicros / 1000).toStringAsFixed(1);
    log?.writeln(
      [
        sent.toUtc().toIso8601String(),
        error == null,
        (rtt.inMicroseconds / 1000).toStringAsFixed(1),
        route,
        csvField(to),
        quicRtt,
        csvField(error ?? ''),
      ].join(','),
    );
    if (!mounted) return;
    setState(() {
      if (error == null) {
        samples.add((route: route, rtt: rtt));
      } else {
        failures++;
      }
      current = to.isEmpty ? route : '$route $to';
    });
  }

  @override
  void dispose() {
    timer?.cancel();
    log?.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final running = timer != null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            FilledButton.tonal(
              key: const Key('measure'),
              onPressed: running ? stop : start,
              child: Text(running ? 'Stop' : 'Measure'),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                current.isEmpty ? '' : 'Path: $current',
                key: const Key('path'),
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
        ),
        if (logPath != null) ...[
          const SizedBox(height: 4),
          Text(summarize(samples, failures), key: const Key('summary')),
          Text('Log: $logPath', style: Theme.of(context).textTheme.bodySmall),
        ],
      ],
    );
  }
}
