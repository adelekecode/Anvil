import 'package:flutter/material.dart';

import '../models/anvil_event.dart';
import '../util/initials.dart';

/// One person in the room.
class ParticipantTile extends StatelessWidget {
  const ParticipantTile({super.key, required this.participant});

  final Participant participant;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;

    return ListTile(
      leading: AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          border: Border.all(
            color: participant.speaking ? scheme.primary : Colors.transparent,
            width: 2,
          ),
        ),
        padding: const EdgeInsets.all(2),
        child: CircleAvatar(
          child: Text(initialOf(participant.displayName)),
        ),
      ),
      title: Text(participant.displayName),
      subtitle: Text(participant.peerId),
      trailing: participant.muted
          ? const Icon(Icons.mic_off, size: 18)
          : participant.speaking
              ? const Icon(Icons.graphic_eq, size: 18)
              : null,
    );
  }
}
