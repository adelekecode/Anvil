// Chat, direct or in a room.
//
// One screen for both, because they are the same thing at different scopes: the
// same peer session, the same encryption, the same reliable-stream delivery.
// Room chat differs only in that it fans out through the relay and can be
// delivered to some members and not others.
//
// The note at the top of an empty conversation is doing deliberate work. Anvil
// has no server to hold a message for someone who is not around, and users have
// no reason to expect that from a messaging app. Saying it once, plainly, up
// front is kinder than letting them discover it from a message that never
// arrives.

import 'package:flutter/material.dart';

import '../models/anvil_event.dart';
import '../state/anvil_controller.dart';
import '../widgets/message_bubble.dart';

class ChatScreen extends StatefulWidget {
  const ChatScreen({
    super.key,
    required this.controller,
    required this.conversation,
    required this.title,
  });

  final AnvilController controller;
  final ConversationRef conversation;
  final String title;

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final _input = TextEditingController();
  final _scroll = ScrollController();

  @override
  void initState() {
    super.initState();
    widget.controller.markRead(widget.conversation);
    widget.controller.addListener(_onChanged);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onChanged);
    _input.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _onChanged() {
    widget.controller.markRead(widget.conversation);
    _scrollToEnd();
  }

  void _scrollToEnd() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_scroll.hasClients) return;
      _scroll.animateTo(
        _scroll.position.maxScrollExtent,
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOut,
      );
    });
  }

  void _send() {
    final body = _input.text.trim();
    if (body.isEmpty) return;

    widget.controller.sendMessage(widget.conversation, body);
    _input.clear();
    _scrollToEnd();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: widget.controller,
      builder: (context, _) {
        final messages = widget.controller.messages(widget.conversation);

        return Scaffold(
          appBar: AppBar(
            title: Text(widget.title),
          ),
          body: Column(
            children: [
              Expanded(
                child: messages.isEmpty
                    ? _EmptyConversation(isDirect: widget.conversation.isDirect)
                    : ListView.builder(
                        controller: _scroll,
                        padding: const EdgeInsets.symmetric(vertical: 12),
                        itemCount: messages.length,
                        itemBuilder: (context, index) {
                          final message = messages[index];
                          return MessageBubble(
                            message: message,
                            senderName: widget.conversation.isDirect
                                ? null
                                : _nameFor(message.from),
                          );
                        },
                      ),
              ),
              _Composer(controller: _input, onSend: _send),
            ],
          ),
        );
      },
    );
  }

  String? _nameFor(String peerId) =>
      widget.controller.peerById(peerId)?.displayName;
}

class _EmptyConversation extends StatelessWidget {
  const _EmptyConversation({required this.isDirect});

  final bool isDirect;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.lock_outline,
                size: 32, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(height: 12),
            Text(
              'Messages are end-to-end encrypted',
              style: theme.textTheme.titleSmall,
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 8),
            Text(
              isDirect
                  ? 'They go straight to the other device. If that person is not '
                      'nearby, the message will not be delivered — there is no '
                      'server holding it for later.'
                  : 'They go to everyone currently in the room. Anyone not '
                      'connected right now will not receive them.',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),
          ],
        ),
      ),
    );
  }
}

class _Composer extends StatelessWidget {
  const _Composer({required this.controller, required this.onSend});

  final TextEditingController controller;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            Expanded(
              child: TextField(
                controller: controller,
                minLines: 1,
                maxLines: 5,
                textInputAction: TextInputAction.send,
                textCapitalization: TextCapitalization.sentences,
                decoration: const InputDecoration(
                  hintText: 'Message…',
                  border: OutlineInputBorder(),
                  isDense: true,
                  contentPadding:
                      EdgeInsets.symmetric(horizontal: 14, vertical: 12),
                ),
                onSubmitted: (_) => onSend(),
              ),
            ),
            const SizedBox(width: 4),
            IconButton.filled(
              icon: const Icon(Icons.send),
              onPressed: onSend,
            ),
          ],
        ),
      ),
    );
  }
}
