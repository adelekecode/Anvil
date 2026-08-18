import 'package:flutter/material.dart';

import '../state/anvil_controller.dart';
import 'fingerprint_view.dart';

/// The identity-changed warning.
///
/// A modal, not a banner, and deliberately hard to dismiss by accident. This is
/// the one moment where Anvil genuinely cannot tell the user what happened, so
/// it has to ask — and phrasing matters more here than anywhere else in the app.
///
/// Three principles behind the wording:
///
/// * **Do not accuse.** The overwhelmingly common cause is a reinstall. Leading
///   with "someone is impersonating Daniel" would cry wolf, and users who are
///   cried wolf at learn to tap through warnings.
/// * **Do not reassure either.** The dangerous case is real. Both explanations
///   are given equal weight, because Anvil genuinely does not know which it is.
/// * **Make the safe action the easy one.** "Check with them" is the primary
///   button. Accepting the change is available, plainly labelled, and does not
///   pretend to be verification.
void showTrustWarningSheet(
  BuildContext context,
  AnvilController controller,
  IdentityWarning warning,
) {
  showModalBottomSheet<void>(
    context: context,
    isDismissible: false,
    enableDrag: false,
    isScrollControlled: true,
    builder: (context) => _TrustWarningSheet(
      controller: controller,
      warning: warning,
    ),
  );
}

class _TrustWarningSheet extends StatelessWidget {
  const _TrustWarningSheet({required this.controller, required this.warning});

  final AnvilController controller;
  final IdentityWarning warning;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(24, 24, 24, 24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.gpp_maybe, color: theme.colorScheme.error),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    "${warning.displayName}'s identity has changed",
                    style: theme.textTheme.titleMedium,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 16),

            Text(
              'A device calling itself ${warning.displayName} is using a '
              'different cryptographic key than the one you saw before.',
              style: theme.textTheme.bodyMedium,
            ),
            const SizedBox(height: 16),

            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(12),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  FingerprintView(
                    label: 'You trusted',
                    fingerprint: warning.previousFingerprint,
                  ),
                  const SizedBox(height: 12),
                  FingerprintView(
                    label: 'Now showing',
                    fingerprint: warning.newFingerprint,
                    emphasis: true,
                  ),
                ],
              ),
            ),
            const SizedBox(height: 16),

            Text('This usually means one of two things:',
                style: theme.textTheme.labelLarge),
            const SizedBox(height: 8),
            const _Explanation(
              icon: Icons.phone_android,
              text: 'They reinstalled Anvil or got a new phone, which creates a '
                  'new identity.',
            ),
            const _Explanation(
              icon: Icons.person_off_outlined,
              text: 'Someone else is using their name.',
            ),
            const SizedBox(height: 8),
            Text(
              'Anvil cannot tell which. Ask them to read out the fingerprint '
              'above — that is the only way to be sure.',
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),

            const SizedBox(height: 24),
            FilledButton.icon(
              onPressed: () {
                controller.verifyPeer(warning.peerId);
                Navigator.of(context).pop();
              },
              icon: const Icon(Icons.check),
              label: const Text('I checked — it matches'),
            ),
            const SizedBox(height: 8),
            TextButton(
              onPressed: () {
                controller.acceptIdentityChange(warning.peerId);
                Navigator.of(context).pop();
              },
              child: const Text('Continue without checking'),
            ),
          ],
        ),
      ),
    );
  }
}

class _Explanation extends StatelessWidget {
  const _Explanation({required this.icon, required this.text});

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 16, color: theme.colorScheme.onSurfaceVariant),
          const SizedBox(width: 10),
          Expanded(child: Text(text, style: theme.textTheme.bodySmall)),
        ],
      ),
    );
  }
}
