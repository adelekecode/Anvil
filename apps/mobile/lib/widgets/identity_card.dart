import 'package:flutter/material.dart';

import '../util/initials.dart';

import '../models/anvil_event.dart';

/// "You" — the top of the home screen.
///
/// Shows the display name, the fingerprint, and whether this device is
/// currently discoverable. The fingerprint is here rather than buried in
/// settings because it is what someone reads aloud when a friend wants to check
/// they are connecting to the right device, and a fingerprint two taps deep is
/// a fingerprint nobody uses.
class IdentityCard extends StatelessWidget {
  const IdentityCard({
    super.key,
    required this.profile,
    required this.discovering,
    this.onRename,
  });

  final Profile profile;
  final bool discovering;
  final VoidCallback? onRename;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
      child: Card(
        margin: EdgeInsets.zero,
        child: InkWell(
          onTap: onRename,
          borderRadius: BorderRadius.circular(12),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Row(
              children: [
                CircleAvatar(
                  radius: 24,
                  child: Text(
                    initialOf(profile.displayName),
                    style: theme.textTheme.titleLarge,
                  ),
                ),
                const SizedBox(width: 16),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(profile.displayName, style: theme.textTheme.titleMedium),
                      const SizedBox(height: 2),
                      Text(
                        profile.fingerprint,
                        style: theme.textTheme.bodySmall?.copyWith(
                          fontFeatures: const [FontFeature.tabularFigures()],
                          color: theme.colorScheme.onSurfaceVariant,
                        ),
                      ),
                      const SizedBox(height: 6),
                      Row(
                        children: [
                          Container(
                            width: 6,
                            height: 6,
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              color: discovering
                                  ? theme.colorScheme.primary
                                  : theme.colorScheme.outline,
                            ),
                          ),
                          const SizedBox(width: 6),
                          Text(
                            discovering ? 'Discoverable nearby' : 'Not discoverable',
                            style: theme.textTheme.bodySmall?.copyWith(
                              color: theme.colorScheme.onSurfaceVariant,
                            ),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
                if (onRename != null)
                  Icon(Icons.edit_outlined,
                      size: 18, color: theme.colorScheme.onSurfaceVariant),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
