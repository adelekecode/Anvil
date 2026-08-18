// App shell: starts the core, holds the controller, decides what is on screen.
//
// Routing here is a single question answered three times, in priority order:
//
// ```text
//   no identity yet?   →  first run
//   a call happening?  →  the call, over everything else
//   otherwise          →  home
// ```
//
// A call outranks everything because that is what a call is. Note that it
// *overlays* rather than replaces — ending a call returns the user exactly where
// they were, which matters when a call interrupts them mid-message.
//
// The Rust node starts once and lives for the app's lifetime. It is deliberately
// not torn down when a room ends: identity and discovery outlive any single
// room, and restarting the engine would regenerate state that is meant to
// persist.

import 'dart:async';

import 'package:flutter/material.dart';

import '../screens/call_screen.dart';
import '../screens/first_run_screen.dart';
import '../screens/home_screen.dart';
import '../services/anvil_api.dart';
import '../state/anvil_controller.dart';

class AnvilApp extends StatefulWidget {
  const AnvilApp({super.key});

  @override
  State<AnvilApp> createState() => _AnvilAppState();
}

class _AnvilAppState extends State<AnvilApp> {
  AnvilApi? _api;
  AnvilController? _controller;
  Object? _startupError;

  @override
  void initState() {
    super.initState();
    _start();
  }

  Future<void> _start() async {
    setState(() => _startupError = null);

    try {
      // The display name here is a placeholder until the profile loads or is
      // created. The core emits ProfileReady either way, and that event — not
      // this value — is what the UI keys off.
      final api = await AnvilApi.start(displayName: 'Anvil');
      if (!mounted) {
        await api.dispose();
        return;
      }
      setState(() {
        _api = api;
        _controller = AnvilController(api);
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _startupError = error);
    }
  }

  @override
  void dispose() {
    _controller?.dispose();
    final api = _api;
    if (api != null) unawaited(api.dispose());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Anvil',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF2B4C7E),
          brightness: Brightness.dark,
        ),
        useMaterial3: true,
      ),
      home: _home(),
    );
  }

  Widget _home() {
    final error = _startupError;
    if (error != null) {
      return _StartupFailure(error: error, onRetry: _start);
    }

    final controller = _controller;
    if (controller == null) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }

    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        if (controller.needsProfile) {
          return FirstRunScreen(controller: controller);
        }

        return Stack(
          children: [
            HomeScreen(controller: controller),
            if (controller.callPhase.isBusy)
              CallScreen(controller: controller),
          ],
        );
      },
    );
  }
}

class _StartupFailure extends StatelessWidget {
  const _StartupFailure({required this.error, required this.onRetry});

  final Object error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.error_outline, size: 48),
              const SizedBox(height: 16),
              Text(
                'Anvil could not start',
                style: Theme.of(context).textTheme.titleLarge,
              ),
              const SizedBox(height: 8),
              Text('$error', textAlign: TextAlign.center),
              const SizedBox(height: 24),
              FilledButton(onPressed: onRetry, child: const Text('Retry')),
            ],
          ),
        ),
      ),
    );
  }
}
