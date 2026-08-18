// First run.
//
// The entire onboarding. One field, one button, no account.
//
// What is deliberately absent: email, password, phone number, verification code,
// terms checkbox, "continue with" buttons, and any network request whatsoever.
// The user types a name and Anvil generates a keypair. Nothing leaves the
// device, and nothing can fail for a reason outside it.
//
// The explanatory line at the bottom is doing real work. People have been
// trained by every other app to expect a signup, and telling them plainly that
// there isn't one — and what that means for recovery — is more honest than
// letting them discover it when they change phones.

import 'package:flutter/material.dart';

import '../state/anvil_controller.dart';

class FirstRunScreen extends StatefulWidget {
  const FirstRunScreen({super.key, required this.controller});

  final AnvilController controller;

  @override
  State<FirstRunScreen> createState() => _FirstRunScreenState();
}

class _FirstRunScreenState extends State<FirstRunScreen> {
  final _name = TextEditingController();
  final _focus = FocusNode();
  bool _submitted = false;

  @override
  void initState() {
    super.initState();
    _name.addListener(() => setState(() {}));
    // Straight into the field: there is nothing else on this screen to read
    // first, and one less tap on the very first interaction is worth having.
    WidgetsBinding.instance.addPostFrameCallback((_) => _focus.requestFocus());
  }

  @override
  void dispose() {
    _name.dispose();
    _focus.dispose();
    super.dispose();
  }

  bool get _canContinue => _name.text.trim().isNotEmpty && !_submitted;

  void _submit() {
    if (!_canContinue) return;
    setState(() => _submitted = true);
    widget.controller.createProfile(_name.text.trim());
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Scaffold(
      body: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 48),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 380),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    'Welcome to Anvil',
                    style: theme.textTheme.headlineSmall,
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'Talk to people nearby. No internet, no accounts.',
                    style: theme.textTheme.bodyMedium
                        ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 40),

                  Text('Display name', style: theme.textTheme.labelLarge),
                  const SizedBox(height: 8),
                  TextField(
                    controller: _name,
                    focusNode: _focus,
                    autofocus: true,
                    enabled: !_submitted,
                    textInputAction: TextInputAction.done,
                    textCapitalization: TextCapitalization.words,
                    maxLength: 48,
                    decoration: const InputDecoration(
                      hintText: 'Femi',
                      border: OutlineInputBorder(),
                      counterText: '',
                    ),
                    onSubmitted: (_) => _submit(),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'This is what nearby people will see. You can change it '
                    'later, and it is not how Anvil identifies you.',
                    style: theme.textTheme.bodySmall
                        ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
                  ),

                  const SizedBox(height: 32),
                  FilledButton(
                    onPressed: _canContinue ? _submit : null,
                    child: _submitted
                        ? const SizedBox(
                            height: 18,
                            width: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Text('Continue'),
                  ),

                  const SizedBox(height: 40),
                  const _NoAccountNote(),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _NoAccountNote extends StatelessWidget {
  const _NoAccountNote();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.lock_outline, size: 16, color: theme.colorScheme.primary),
              const SizedBox(width: 8),
              Text('There is no account', style: theme.textTheme.labelLarge),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            'Anvil creates a cryptographic identity on this device. It never '
            'leaves. Nothing is sent anywhere, and there is nothing to sign in '
            'to — which also means nobody can restore it for you if you lose '
            'this phone.',
            style: theme.textTheme.bodySmall
                ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}
