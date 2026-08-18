// A room: who is in it, the code to get others in, voice and chat.
//
// The join code is the most important thing on this screen when the room is
// empty, and almost irrelevant once it is full — so it is prominent at first
// and collapses into the app bar afterwards. Getting people in is the whole job
// of an empty room.
//
// What the screen does not show: which transport carries the audio, or which
// device is relaying. Both are in Diagnostics. The product promise is that the
// user does not have to know, and putting a relay indicator here would invite
// them to worry about a decision the engine is making correctly.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models/anvil_event.dart';
import '../state/anvil_controller.dart';
import '../widgets/error_banner.dart';
import '../widgets/participant_tile.dart';
import 'chat_screen.dart';
import 'diagnostics_screen.dart';

class RoomScreen extends StatelessWidget {
  const RoomScreen({super.key, required this.controller});

  final AnvilController controller;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final room = controller.room;

        if (room == null) {
          return Scaffold(
            appBar: AppBar(),
            body: const Center(child: Text('You are not in a room')),
          );
        }

        final conversation = ConversationRef.room(room.roomId);
        final unread = controller.unread(conversation);
        final alone = room.participants.length <= 1;

        return Scaffold(
          appBar: AppBar(
            title: Text('Room ${room.shortId}'),
            actions: [
              IconButton(
                icon: Badge(
                  isLabelVisible: unread > 0,
                  label: Text('$unread'),
                  child: const Icon(Icons.chat_bubble_outline),
                ),
                tooltip: 'Room chat',
                onPressed: () => Navigator.of(context).push(
                  MaterialPageRoute<void>(
                    builder: (_) => ChatScreen(
                      controller: controller,
                      conversation: conversation,
                      title: 'Room ${room.shortId}',
                    ),
                  ),
                ),
              ),
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
          body: Column(
            children: [
              ErrorBanner(controller: controller),
              if (controller.state.isUnsettled)
                _UnsettledBanner(state: controller.state),
              if (room.joinCode != null) _JoinCodeCard(code: room.joinCode!, prominent: alone),
              Expanded(
                child: alone
                    ? const _WaitingForOthers()
                    : ListView(
                        children: [
                          for (final participant in room.participants)
                            ParticipantTile(participant: participant),
                        ],
                      ),
              ),
              _Controls(controller: controller),
            ],
          ),
        );
      },
    );
  }
}

class _JoinCodeCard extends StatelessWidget {
  const _JoinCodeCard({required this.code, required this.prominent});

  final String code;
  final bool prominent;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Container(
      width: double.infinity,
      margin: const EdgeInsets.fromLTRB(16, 12, 16, 4),
      padding: EdgeInsets.symmetric(
        horizontal: 20,
        vertical: prominent ? 24 : 14,
      ),
      decoration: BoxDecoration(
        color: theme.colorScheme.primaryContainer,
        borderRadius: BorderRadius.circular(16),
      ),
      child: Column(
        children: [
          Text(
            'ROOM CODE',
            style: theme.textTheme.labelSmall?.copyWith(
              letterSpacing: 1.4,
              color: theme.colorScheme.onPrimaryContainer,
            ),
          ),
          const SizedBox(height: 6),
          SelectableText(
            code,
            style: (prominent
                    ? theme.textTheme.headlineMedium
                    : theme.textTheme.titleLarge)
                ?.copyWith(
              color: theme.colorScheme.onPrimaryContainer,
              letterSpacing: 2,
              fontWeight: FontWeight.w600,
            ),
          ),
          if (prominent) ...[
            const SizedBox(height: 4),
            Text(
              'Read this out, or share it',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onPrimaryContainer,
              ),
            ),
            const SizedBox(height: 12),
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                TextButton.icon(
                  onPressed: () async {
                    await Clipboard.setData(ClipboardData(text: code));
                    if (!context.mounted) return;
                    ScaffoldMessenger.of(context).showSnackBar(
                      const SnackBar(content: Text('Room code copied')),
                    );
                  },
                  icon: const Icon(Icons.copy, size: 18),
                  label: const Text('Copy'),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _WaitingForOthers extends StatelessWidget {
  const _WaitingForOthers();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const SizedBox(
              width: 22,
              height: 22,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(height: 16),
            Text('Waiting for others', style: theme.textTheme.titleSmall),
            const SizedBox(height: 6),
            Text(
              'Anyone nearby who enters the code joins this room. Nothing about '
              'it is stored anywhere — the room exists only on the devices in it.',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),
          ],
        ),
      ),
    );
  }
}

class _UnsettledBanner extends StatelessWidget {
  const _UnsettledBanner({required this.state});

  final AnvilState state;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    final (message, detail) = switch (state) {
      AnvilState.reconnecting => (
          'Reconnecting',
          'The network changed. The room is still here.',
        ),
      AnvilState.relayElection => (
          'Reorganising',
          'Picking a new device to carry the room.',
        ),
      _ => ('', ''),
    };

    return Material(
      color: theme.colorScheme.secondaryContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            const SizedBox(
              width: 16,
              height: 16,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(message, style: theme.textTheme.labelLarge),
                  Text(detail, style: theme.textTheme.bodySmall),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _Controls extends StatelessWidget {
  const _Controls({required this.controller});

  final AnvilController controller;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.spaceEvenly,
          children: [
            _CircleButton(
              icon: controller.muted ? Icons.mic_off : Icons.mic,
              label: controller.muted ? 'Unmute' : 'Mute',
              selected: controller.muted,
              onPressed: controller.toggleMute,
            ),
            _CircleButton(
              icon: Icons.call_end,
              label: 'Leave',
              destructive: true,
              onPressed: () {
                controller.leaveRoom();
                Navigator.of(context).maybePop();
              },
            ),
          ],
        ),
      ),
    );
  }
}

class _CircleButton extends StatelessWidget {
  const _CircleButton({
    required this.icon,
    required this.label,
    required this.onPressed,
    this.selected = false,
    this.destructive = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback onPressed;
  final bool selected;
  final bool destructive;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final background = destructive
        ? scheme.errorContainer
        : selected
            ? scheme.primaryContainer
            : scheme.surfaceContainerHighest;

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Material(
          color: background,
          shape: const CircleBorder(),
          child: InkWell(
            customBorder: const CircleBorder(),
            onTap: onPressed,
            child: Padding(
              padding: const EdgeInsets.all(20),
              child: Icon(icon, size: 28),
            ),
          ),
        ),
        const SizedBox(height: 8),
        Text(label, style: Theme.of(context).textTheme.labelMedium),
      ],
    );
  }
}
