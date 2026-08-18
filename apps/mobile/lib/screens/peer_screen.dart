// One peer: call them, message them, check who they are.
//
// The two actions are given equal weight. Calling and messaging ride the same
// authenticated session, so neither is a lesser feature — and in a room full of
// people, typing is often the polite one.
//
// The identity section is not buried in a submenu. Anvil's whole trust model is
// "you can see who you are talking to", and hiding the fingerprint behind a tap
// would make the one thing a user can actually verify the one thing they never
// find.

import 'package:flutter/material.dart';

import '../models/anvil_event.dart';
import '../state/anvil_controller.dart';
import '../util/initials.dart';
import '../widgets/fingerprint_view.dart';
import 'chat_screen.dart';

class PeerScreen extends StatelessWidget {
  const PeerScreen({
    super.key,
    required this.controller,
    required this.fingerprint,
  });

  final AnvilController controller;

  /// Identified by discovery fingerprint rather than peer id, because a peer
  /// may not be cryptographically confirmed yet — and this screen has to be
  /// able to show that state honestly.
  final String fingerprint;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final peer = controller.peerByFingerprint(fingerprint);

        if (peer == null) {
          return Scaffold(
            appBar: AppBar(),
            body: const Center(child: Text('This device is no longer nearby')),
          );
        }

        return Scaffold(
          appBar: AppBar(title: Text(peer.displayName)),
          body: ListView(
            padding: const EdgeInsets.only(bottom: 32),
            children: [
              _Header(peer: peer),
              if (peer.needsWarning) _IdentityChangedBanner(peer: peer),
              _Actions(controller: controller, peer: peer),
              const Divider(height: 32),
              _Identity(peer: peer, controller: controller),
            ],
          ),
        );
      },
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.peer});

  final DiscoveredPeer peer;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 24, 24, 8),
      child: Column(
        children: [
          CircleAvatar(
            radius: 36,
            child: Text(
              initialOf(peer.displayName),
              style: theme.textTheme.headlineMedium,
            ),
          ),
          const SizedBox(height: 12),
          Text(peer.displayName, style: theme.textTheme.titleLarge),
          const SizedBox(height: 4),
          Text(
            peer.rttMs == null
                ? 'Connected directly'
                : 'Connected directly · ${peer.rttMs} ms',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

class _Actions extends StatelessWidget {
  const _Actions({required this.controller, required this.peer});

  final AnvilController controller;
  final DiscoveredPeer peer;

  @override
  Widget build(BuildContext context) {
    // Both actions need a confirmed identity: there is nobody to call or
    // message until the handshake has proved who this is.
    final peerId = peer.peerId;
    final ready = peerId != null;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          FilledButton.icon(
            onPressed: ready ? () => controller.call(peerId) : null,
            icon: const Icon(Icons.call),
            label: const Text('Voice call'),
          ),
          const SizedBox(height: 12),
          OutlinedButton.icon(
            onPressed: ready
                ? () => Navigator.of(context).push(
                      MaterialPageRoute<void>(
                        builder: (_) => ChatScreen(
                          controller: controller,
                          conversation: ConversationRef.direct(peerId),
                          title: peer.displayName,
                        ),
                      ),
                    )
                : null,
            icon: const Icon(Icons.chat_bubble_outline),
            label: const Text('Message'),
          ),
          if (!ready) ...[
            const SizedBox(height: 8),
            Text(
              'Still establishing a secure session with this device.',
              textAlign: TextAlign.center,
              style: Theme.of(context)
                  .textTheme
                  .bodySmall
                  ?.copyWith(color: Theme.of(context).colorScheme.onSurfaceVariant),
            ),
          ],
        ],
      ),
    );
  }
}

class _IdentityChangedBanner extends StatelessWidget {
  const _IdentityChangedBanner({required this.peer});

  final DiscoveredPeer peer;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Container(
      margin: const EdgeInsets.fromLTRB(24, 8, 24, 8),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: theme.colorScheme.errorContainer,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.gpp_maybe, color: theme.colorScheme.onErrorContainer),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              'This device is using a different key than the '
              '${peer.displayName} you saw before. Check the fingerprint with '
              'them before saying anything sensitive.',
              style: TextStyle(color: theme.colorScheme.onErrorContainer),
            ),
          ),
        ],
      ),
    );
  }
}

class _Identity extends StatelessWidget {
  const _Identity({required this.peer, required this.controller});

  final DiscoveredPeer peer;
  final AnvilController controller;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final trust = peer.trust;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('IDENTITY',
              style: theme.textTheme.labelSmall?.copyWith(
                letterSpacing: 1.2,
                color: theme.colorScheme.onSurfaceVariant,
              )),
          const SizedBox(height: 12),

          FingerprintView(
            label: 'Fingerprint',
            fingerprint: peer.fingerprint.toUpperCase(),
            emphasis: true,
          ),
          const SizedBox(height: 16),

          _TrustRow(trust: trust, confirmed: peer.confirmed),
          const SizedBox(height: 16),

          Text(
            'Anvil identifies devices by their cryptographic key, not by name. '
            'Two people can both be called "${peer.displayName}"; only the '
            'fingerprint tells them apart.',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),

          if (peer.peerId != null && trust != PeerTrust.verified) ...[
            const SizedBox(height: 16),
            OutlinedButton.icon(
              onPressed: () => controller.verifyPeer(peer.peerId!),
              icon: const Icon(Icons.verified_outlined),
              label: const Text('I checked this fingerprint in person'),
            ),
          ],
        ],
      ),
    );
  }
}

class _TrustRow extends StatelessWidget {
  const _TrustRow({required this.trust, required this.confirmed});

  final PeerTrust? trust;
  final bool confirmed;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    final (icon, label, detail, colour) = switch (trust) {
      PeerTrust.verified => (
          Icons.verified_user,
          'Verified',
          'You confirmed this key in person.',
          theme.colorScheme.primary,
        ),
      PeerTrust.changed => (
          Icons.gpp_maybe,
          'Identity changed',
          'A different key than you saw before.',
          theme.colorScheme.error,
        ),
      PeerTrust.unverified => (
          Icons.shield_outlined,
          'Known, not verified',
          'Same device as last time. Not checked in person.',
          theme.colorScheme.onSurfaceVariant,
        ),
      null => (
          confirmed ? Icons.shield_outlined : Icons.help_outline,
          confirmed ? 'New device' : 'Not yet verified',
          confirmed
              ? 'First time you have met this device.'
              : 'This name is unconfirmed until a secure session is established.',
          theme.colorScheme.onSurfaceVariant,
        ),
    };

    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(icon, size: 18, color: colour),
        const SizedBox(width: 10),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(label,
                  style: theme.textTheme.labelLarge?.copyWith(color: colour)),
              Text(detail,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  )),
            ],
          ),
        ),
      ],
    );
  }
}
