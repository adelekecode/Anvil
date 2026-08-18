import 'package:flutter/material.dart';

import '../state/anvil_controller.dart';

/// Shows the most recent error, with the layer that produced it.
///
/// Naming the layer is the point (§91): "transport: no path to peer" tells
/// someone something they can act on. "Connection error" does not.
class ErrorBanner extends StatelessWidget {
  const ErrorBanner({super.key, required this.controller});

  final AnvilController controller;

  @override
  Widget build(BuildContext context) {
    final error = controller.lastError;
    if (error == null) return const SizedBox.shrink();

    final scheme = Theme.of(context).colorScheme;

    return Material(
      color: scheme.errorContainer,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
        child: Row(
          children: [
            Icon(Icons.error_outline, size: 18, color: scheme.onErrorContainer),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                error,
                style: TextStyle(color: scheme.onErrorContainer),
              ),
            ),
            IconButton(
              icon: const Icon(Icons.close, size: 18),
              onPressed: controller.clearError,
            ),
          ],
        ),
      ),
    );
  }
}
