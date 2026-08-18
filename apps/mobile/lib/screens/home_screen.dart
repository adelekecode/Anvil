// The home surface.
//
// One screen, three sections: who you are, who is nearby, and rooms. No tabs —
// the whole product fits on one page at this stage, and splitting it would make
// people navigate to find out whether anyone is around, which is the single
// question they open the app to answer.
//
// ```text
//   YOU      Femi · 7A:42:19:BC · discoverable
//   NEARBY   Daniel 4ms · Sarah 8ms · Michael 12ms
//   ROOMS    [ Create room ]  [ ANV-____-____  Join ]
// ```
//
// Note what the Nearby list does not show: which transport carries each peer,
// or whether a relay is involved. That is the product's central promise — the
// user should not have to know — and it lives in Diagnostics for when it
// matters.

import 'package:flutter/material.dart';

import '../models/anvil_event.dart';
import '../state/anvil_controller.dart';
import '../widgets/error_banner.dart';
import '../widgets/identity_card.dart';
import '../widgets/peer_tile.dart';
import '../widgets/trust_warning_sheet.dart';
import 'diagnostics_screen.dart';
import 'peer_screen.dart';
import 'room_screen.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.controller});

  final AnvilController controller;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  final _joinCode = TextEditingController();
  bool _warningShown = false;

  @override
  void initState() {
    super.initState();
    // Opening Anvil is what makes you discoverable. There is no separate
    // "go online" step, because there is nothing to go online to.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      widget.controller.startDiscovery();
    });
    widget.controller.addListener(_maybeShowWarning);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_maybeShowWarning);
    _joinCode.dispose();
    super.dispose();
  }

  /// A changed identity is a decision, not a notification. It gets a modal.
  void _maybeShowWarning() {
    final warning = widget.controller.identityWarning;
    if (warning == null) {
      _warningShown = false;
      return;
    }
    if (_warningShown || !mounted) return;

    _warningShown = true;
    showTrustWarningSheet(context, widget.controller, warning);
  }

  void _join() {
    final code = _joinCode.text.trim();
    if (code.isEmpty) return;
    widget.controller.joinRoomByCode(code);
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final peers = controller.peers;

    return Scaffold(
      appBar: AppBar(
        title: const Text('Anvil'),
        actions: [
          IconButton(
            icon: const Icon(Icons.insights_outlined),
            tooltip: 'Diagnostics',
            onPressed: () => Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => DiagnosticsScreen(controller: controller),
              ),
            ),
          ),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.only(bottom: 32),
        children: [
          ErrorBanner(controller: controller),

          if (controller.profile != null)
            IdentityCard(
              profile: controller.profile!,
              discovering: controller.state == AnvilState.discovering,
              onRename: () => _showRename(context),
            ),

          _SectionHeader(
            title: 'Nearby',
            trailing: peers.isEmpty ? null : '${peers.length}',
            action: controller.state == AnvilState.discovering
                ? _SectionAction('Stop', controller.stopDiscovery)
                : _SectionAction('Search', controller.startDiscovery),
          ),

          if (peers.isEmpty)
            const _NobodyNearby()
          else
            for (final peer in peers)
              PeerTile(
                peer: peer,
                onTap: () => _openPeer(context, peer),
                onCall: peer.peerId == null ? null : () => controller.call(peer.peerId!),
              ),

          const SizedBox(height: 8),
          const _SectionHeader(title: 'Rooms'),
          _RoomActions(
            controller: controller,
            joinCode: _joinCode,
            onJoin: _join,
          ),
        ],
      ),
    );
  }

  void _openPeer(BuildContext context, DiscoveredPeer peer) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => PeerScreen(
          controller: widget.controller,
          fingerprint: peer.fingerprint,
        ),
      ),
    );
  }

  Future<void> _showRename(BuildContext context) async {
    final controller = TextEditingController(
      text: widget.controller.profile?.displayName ?? '',
    );

    final name = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Display name'),
        content: TextField(
          controller: controller,
          autofocus: true,
          maxLength: 48,
          decoration: const InputDecoration(counterText: ''),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(controller.text.trim()),
            child: const Text('Save'),
          ),
        ],
      ),
    );

    controller.dispose();
    if (name != null && name.isNotEmpty) {
      widget.controller.renameProfile(name);
    }
  }
}

class _SectionAction {
  const _SectionAction(this.label, this.onPressed);
  final String label;
  final VoidCallback onPressed;
}

class _SectionHeader extends StatelessWidget {
  const _SectionHeader({required this.title, this.trailing, this.action});

  final String title;
  final String? trailing;
  final _SectionAction? action;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 20, 8, 4),
      child: Row(
        children: [
          Text(
            title.toUpperCase(),
            style: theme.textTheme.labelSmall?.copyWith(
              letterSpacing: 1.2,
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          if (trailing != null) ...[
            const SizedBox(width: 8),
            Text(
              trailing!,
              style: theme.textTheme.labelSmall
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),
          ],
          const Spacer(),
          if (action != null)
            TextButton(onPressed: action!.onPressed, child: Text(action!.label)),
        ],
      ),
    );
  }
}

class _NobodyNearby extends StatelessWidget {
  const _NobodyNearby();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 24, 16, 24),
      child: Column(
        children: [
          Icon(
            Icons.groups_outlined,
            size: 40,
            color: theme.colorScheme.onSurfaceVariant,
          ),
          const SizedBox(height: 12),
          Text('Nobody nearby yet', style: theme.textTheme.titleSmall),
          const SizedBox(height: 6),
          Text(
            'Anvil finds people on the same Wi-Fi, or directly device to device '
            'when there is no router. Neither needs internet.',
            textAlign: TextAlign.center,
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

class _RoomActions extends StatelessWidget {
  const _RoomActions({
    required this.controller,
    required this.joinCode,
    required this.onJoin,
  });

  final AnvilController controller;
  final TextEditingController joinCode;
  final VoidCallback onJoin;

  @override
  Widget build(BuildContext context) {
    final room = controller.room;

    if (room != null) {
      return Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16),
        child: Card(
          margin: EdgeInsets.zero,
          child: ListTile(
            leading: const Icon(Icons.meeting_room),
            title: Text('Room ${room.shortId}'),
            subtitle: Text('${room.participants.length} in the room'),
            trailing: FilledButton(
              onPressed: () => Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) => RoomScreen(controller: controller),
                ),
              ),
              child: const Text('Open'),
            ),
          ),
        ),
      );
    }

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          FilledButton.icon(
            onPressed: controller.createRoom,
            icon: const Icon(Icons.add),
            label: const Text('Create room'),
          ),
          const SizedBox(height: 16),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: TextField(
                  controller: joinCode,
                  textCapitalization: TextCapitalization.characters,
                  decoration: const InputDecoration(
                    labelText: 'Room code',
                    hintText: 'ANV-7FK2-P9W4',
                    border: OutlineInputBorder(),
                    isDense: true,
                  ),
                  onSubmitted: (_) => onJoin(),
                ),
              ),
              const SizedBox(width: 12),
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: FilledButton.tonal(
                  onPressed: onJoin,
                  child: const Text('Join'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            'Codes are not case sensitive, and hyphens are optional.',
            style: Theme.of(context)
                .textTheme
                .bodySmall
                ?.copyWith(color: Theme.of(context).colorScheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}
