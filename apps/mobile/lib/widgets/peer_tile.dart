import 'package:flutter/material.dart';

import '../models/anvil_event.dart';
import '../util/initials.dart';

/// One nearby device.
///
/// Three signals, in order of how much they matter:
///
/// * **A trust warning**, if this peer presented a different key for a name we
///   already trusted. Loud, red, unmissable.
/// * **Known or new.** A filled star for someone met before. Not a security
///   claim — just "you have talked to this device".
/// * **Unverified name.** Until the handshake completes, the name is a claim by
///   whoever is broadcasting. Marked, because presenting it as a person would
///   be actively misleading.
///
/// Latency is shown because it is the one network fact a user can act on — if
/// someone reads as 200 ms in the same room, something is wrong. Which
/// transport carries them is deliberately not shown; that is the whole point of
/// adaptive transport.
class PeerTile extends StatelessWidget {
  const PeerTile({
    super.key,
    required this.peer,
    this.onTap,
    this.onCall,
  });

  final DiscoveredPeer peer;
  final VoidCallback? onTap;
  final VoidCallback? onCall;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return ListTile(
      onTap: onTap,
      leading: Stack(
        clipBehavior: Clip.none,
        children: [
          CircleAvatar(
            backgroundColor: peer.needsWarning
                ? theme.colorScheme.errorContainer
                : theme.colorScheme.secondaryContainer,
            child: peer.needsWarning
                ? Icon(Icons.priority_high, color: theme.colorScheme.onErrorContainer)
                : Text(initialOf(peer.displayName)),
          ),
          if (peer.known && !peer.needsWarning)
            Positioned(
              right: -2,
              bottom: -2,
              child: Icon(Icons.star, size: 14, color: theme.colorScheme.primary),
            ),
        ],
      ),
      title: Row(
        children: [
          Flexible(
            child: Text(peer.displayName, overflow: TextOverflow.ellipsis),
          ),
          if (!peer.confirmed) ...[
            const SizedBox(width: 6),
            Tooltip(
              message: 'Name not verified yet',
              child: Icon(Icons.help_outline,
                  size: 14, color: theme.colorScheme.outline),
            ),
          ],
        ],
      ),
      subtitle: Text(
        peer.needsWarning
            ? 'Identity changed'
            : peer.hostingRoom
                ? 'Hosting a room'
                : peer.known
                    ? 'Known device'
                    : 'New device',
        style: peer.needsWarning
            ? TextStyle(color: theme.colorScheme.error)
            : null,
      ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            peer.rttMs == null ? '—' : '${peer.rttMs} ms',
            style: theme.textTheme.bodySmall?.copyWith(
              fontFeatures: const [FontFeature.tabularFigures()],
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          if (onCall != null) ...[
            const SizedBox(width: 4),
            IconButton(
              icon: const Icon(Icons.call),
              tooltip: 'Call ${peer.displayName}',
              onPressed: onCall,
            ),
          ],
        ],
      ),
    );
  }
}
