import 'package:flutter_test/flutter_test.dart';
import 'package:kuri_link_example/probe.dart';

Duration ms(int n) => Duration(milliseconds: n);

void main() {
  test('percentiles are by nearest rank', () {
    final sorted = [for (var i = 1; i <= 20; i++) ms(i)];
    expect(percentile(sorted, 50), ms(10));
    expect(percentile(sorted, 95), ms(19));
    expect(percentile(sorted, 100), ms(20));
    expect(percentile([ms(7)], 95), ms(7));
  });

  test('the summary splits calls by route', () {
    final samples = [
      for (var i = 1; i <= 3; i++) (route: 'direct', rtt: ms(i)),
      (route: 'relay', rtt: ms(40)),
    ];
    expect(
      summarize(samples, 2),
      'direct: 3 calls (75%), p50 2 ms, p95 3 ms\n'
      'relay: 1 calls (25%), p50 40 ms, p95 40 ms\n'
      'failed: 2',
    );
  });
}
