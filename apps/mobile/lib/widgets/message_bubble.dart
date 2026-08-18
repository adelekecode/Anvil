import 'package:flutter/material.dart';

import '../models/anvil_event.dart';

/// One message.
///
/// Delivery state is shown honestly. With no server there is nothing to hold a
/// message for an absent peer, so "undeliverable" is a real and reasonably
/// common outcome — showing a hopeful clock icon forever would be a lie, and
/// users would learn not to trust any of the indicators.
class MessageBubble extends StatelessWidget {
  const MessageBubble({super.key, required this.message, this.senderName});

  final ChatMessage message;
  final String? senderName;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final outbound = message.outbound;
    final failed = message.delivery.isFailure;

    final background = failed
        ? theme.colorScheme.errorContainer
        : outbound
            ? theme.colorScheme.primaryContainer
            : theme.colorScheme.surfaceContainerHighest;

    return Align(
      alignment: outbound ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: MediaQuery.sizeOf(context).width * 0.78,
        ),
        child: Container(
          margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 3),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          decoration: BoxDecoration(
            color: background,
            borderRadius: BorderRadius.circular(16),
          ),
          child: Column(
            crossAxisAlignment:
                outbound ? CrossAxisAlignment.end : CrossAxisAlignment.start,
            children: [
              if (!outbound && senderName != null) ...[
                Text(senderName!, style: theme.textTheme.labelSmall),
                const SizedBox(height: 2),
              ],
              Text(message.body),
              if (outbound) ...[
                const SizedBox(height: 4),
                _DeliveryLabel(message: message),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _DeliveryLabel extends StatelessWidget {
  const _DeliveryLabel({required this.message});

  final ChatMessage message;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    final (icon, text) = switch (message.delivery) {
      MessageDelivery.pending => (Icons.schedule, 'Sending'),
      MessageDelivery.sent => (Icons.check, 'Sent'),
      MessageDelivery.delivered => (Icons.done_all, 'Delivered'),
      MessageDelivery.undeliverable => (
          Icons.error_outline,
          'Not delivered — they were not reachable'
        ),
      MessageDelivery.partial => (
          Icons.done_all,
          'Delivered to ${message.deliveredCount} of ${message.totalCount}'
        ),
    };

    final colour = message.delivery.isFailure
        ? theme.colorScheme.onErrorContainer
        : theme.colorScheme.onSurfaceVariant;

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 12, color: colour),
        const SizedBox(width: 4),
        Flexible(
          child: Text(
            text,
            style: theme.textTheme.labelSmall?.copyWith(color: colour),
          ),
        ),
      ],
    );
  }
}
