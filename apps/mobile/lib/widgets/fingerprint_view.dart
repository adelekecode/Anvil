import 'package:flutter/material.dart';

/// A fingerprint, laid out to be compared by eye.
///
/// Monospaced digits and generous spacing, because the entire value of a
/// fingerprint is that two people can look at two screens and agree. Cramped
/// proportional text defeats that.
class FingerprintView extends StatelessWidget {
  const FingerprintView({
    super.key,
    required this.fingerprint,
    this.label,
    this.emphasis = false,
  });

  final String fingerprint;
  final String? label;
  final bool emphasis;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (label != null) ...[
          Text(
            label!,
            style: theme.textTheme.labelSmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
          const SizedBox(height: 4),
        ],
        Text(
          fingerprint,
          style: (emphasis
                  ? theme.textTheme.titleMedium
                  : theme.textTheme.bodyMedium)
              ?.copyWith(
            fontFeatures: const [FontFeature.tabularFigures()],
            letterSpacing: 1.5,
          ),
        ),
      ],
    );
  }
}
