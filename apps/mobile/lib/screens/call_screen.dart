// Direct calls: ringing out, ringing in, connected.
//
// One screen for all three states rather than three screens, because they are
// one continuous moment for the user and animating between separate routes
// would make a fast answer feel jumpy.
//
// This is presented as a full-screen overlay above whatever the user was doing.
// An incoming call has to interrupt — that is what a call is — but declining it
// must put them back exactly where they were, not on the home screen.

import 'package:flutter/material.dart';

import '../models/anvil_event.dart';
import '../state/anvil_controller.dart';
import '../util/initials.dart';

class CallScreen extends StatelessWidget {
  const CallScreen({super.key, required this.controller});

  final AnvilController controller;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final phase = controller.callPhase;
    final name = controller.callPeerName;

    return Scaffold(
      backgroundColor: theme.colorScheme.surface,
      body: SafeArea(
        child: Column(
          children: [
            const Spacer(flex: 2),
            _Caller(name: name, phase: phase),
            const Spacer(flex: 3),
            _Controls(controller: controller, phase: phase),
            const SizedBox(height: 48),
          ],
        ),
      ),
    );
  }
}

class _Caller extends StatelessWidget {
  const _Caller({required this.name, required this.phase});

  final String name;
  final CallPhase phase;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    final status = switch (phase) {
      CallPhase.outgoing => 'Calling…',
      CallPhase.incoming => 'Incoming call',
      CallPhase.active => 'Connected',
      CallPhase.idle => '',
    };

    return Column(
      children: [
        CircleAvatar(
          radius: 52,
          child: Text(
            initialOf(name),
            style: theme.textTheme.displaySmall,
          ),
        ),
        const SizedBox(height: 24),
        Text(name.isEmpty ? 'Unknown' : name, style: theme.textTheme.headlineSmall),
        const SizedBox(height: 8),
        Text(
          status,
          style: theme.textTheme.bodyLarge
              ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
        ),
        if (phase == CallPhase.active) ...[
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(Icons.lock, size: 13, color: theme.colorScheme.primary),
              const SizedBox(width: 6),
              Text(
                'End-to-end encrypted · direct',
                style: theme.textTheme.bodySmall
                    ?.copyWith(color: theme.colorScheme.primary),
              ),
            ],
          ),
        ],
      ],
    );
  }
}

class _Controls extends StatelessWidget {
  const _Controls({required this.controller, required this.phase});

  final AnvilController controller;
  final CallPhase phase;

  @override
  Widget build(BuildContext context) {
    // Ringing in: decline and accept, well separated so a fumbled tap in a
    // pocket does not answer a call.
    if (phase == CallPhase.incoming) {
      return Row(
        mainAxisAlignment: MainAxisAlignment.spaceEvenly,
        children: [
          _CallButton(
            icon: Icons.call_end,
            label: 'Decline',
            destructive: true,
            onPressed: controller.declineCall,
          ),
          _CallButton(
            icon: Icons.call,
            label: 'Accept',
            affirmative: true,
            onPressed: controller.acceptCall,
          ),
        ],
      );
    }

    // Ringing out, or connected.
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceEvenly,
      children: [
        if (phase == CallPhase.active)
          _CallButton(
            icon: controller.muted ? Icons.mic_off : Icons.mic,
            label: controller.muted ? 'Unmute' : 'Mute',
            selected: controller.muted,
            onPressed: controller.toggleMute,
          ),
        _CallButton(
          icon: Icons.call_end,
          label: phase == CallPhase.outgoing ? 'Cancel' : 'End',
          destructive: true,
          onPressed: controller.endCall,
        ),
      ],
    );
  }
}

class _CallButton extends StatelessWidget {
  const _CallButton({
    required this.icon,
    required this.label,
    required this.onPressed,
    this.destructive = false,
    this.affirmative = false,
    this.selected = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback onPressed;
  final bool destructive;
  final bool affirmative;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;

    final background = destructive
        ? scheme.error
        : affirmative
            ? scheme.primary
            : selected
                ? scheme.primaryContainer
                : scheme.surfaceContainerHighest;

    final foreground = destructive
        ? scheme.onError
        : affirmative
            ? scheme.onPrimary
            : selected
                ? scheme.onPrimaryContainer
                : scheme.onSurface;

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
              padding: const EdgeInsets.all(22),
              child: Icon(icon, size: 30, color: foreground),
            ),
          ),
        ),
        const SizedBox(height: 10),
        Text(label, style: Theme.of(context).textTheme.labelMedium),
      ],
    );
  }
}
