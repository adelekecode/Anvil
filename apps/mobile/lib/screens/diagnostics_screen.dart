// Diagnostics (§92).
//
// Exists from the first prototype on purpose: the failures this system will
// actually have — a path switching too often, a relay losing elections, a
// jitter buffer pinned at its ceiling — are invisible from the room screen. All
// a user can report otherwise is "it sounded bad".
//
// Nothing sensitive is shown here, because nothing sensitive is *sent* here:
// the core's snapshot type has no field capable of holding key material.

import 'package:flutter/material.dart';

import '../state/anvil_controller.dart';

class DiagnosticsScreen extends StatefulWidget {
  const DiagnosticsScreen({super.key, required this.controller});

  final AnvilController controller;

  @override
  State<DiagnosticsScreen> createState() => _DiagnosticsScreenState();
}

class _DiagnosticsScreenState extends State<DiagnosticsScreen> {
  @override
  void initState() {
    super.initState();
    widget.controller.requestDiagnostics();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Diagnostics'),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh),
            onPressed: widget.controller.requestDiagnostics,
          ),
        ],
      ),
      body: AnimatedBuilder(
        animation: widget.controller,
        builder: (context, _) {
          final data = widget.controller.diagnostics;
          if (data == null) {
            return const Center(child: Text('No snapshot yet'));
          }

          final paths = (data['paths'] as List?) ?? const [];

          return ListView(
            padding: const EdgeInsets.all(16),
            children: [
              _Section('Identity', {
                'Peer': data['localPeer'],
                'Room': data['room'] ?? '—',
                'Relay': data['relay'] ?? '—',
                'Relaying': data['isRelay'] == true ? 'yes' : 'no',
                'Key epoch': data['epoch'],
                'Participants': data['participants'],
              }),
              _Section('Media', {
                'Opus bitrate': '${data['opusBitrateBps']} bps',
                'Packets sent': data['packetsSent'],
                'Packets received': data['packetsReceived'],
                'Frames concealed': data['framesConcealed'],
              }),
              _Section('Rejected', {
                'Failed authentication': data['packetsRejectedAuth'],
                'Replays': data['packetsRejectedReplay'],
              }),
              _Section('Stability', {
                'Path switches': data['pathSwitches'],
                'Relay changes': data['relayChanges'],
              }),
              if (paths.isNotEmpty) ...[
                const SizedBox(height: 16),
                Text('Paths', style: Theme.of(context).textTheme.titleMedium),
                for (final path in paths.cast<Map<String, dynamic>>())
                  ListTile(
                    dense: true,
                    leading: Icon(
                      path['active'] == true
                          ? Icons.radio_button_checked
                          : Icons.radio_button_unchecked,
                      size: 18,
                    ),
                    title: Text('${path['kind']}'),
                    subtitle: Text(
                      'rtt ${path['rttMs']}ms · '
                      'loss ${((path['loss'] as num?) ?? 0) * 100}% · '
                      'jitter ${path['jitterMs']}ms',
                    ),
                    trailing: Text('${path['score']}'),
                  ),
              ],
            ],
          );
        },
      ),
    );
  }
}

class _Section extends StatelessWidget {
  const _Section(this.title, this.rows);

  final String title;
  final Map<String, Object?> rows;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 8),
        Text(title, style: Theme.of(context).textTheme.titleMedium),
        const SizedBox(height: 4),
        for (final entry in rows.entries)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 2),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(entry.key,
                    style: Theme.of(context).textTheme.bodyMedium),
                Text('${entry.value}',
                    style: Theme.of(context).textTheme.bodyMedium),
              ],
            ),
          ),
        const Divider(height: 24),
      ],
    );
  }
}
