//! Local conversation history.
//!
//! Held on the device that participated, and nowhere else. There is no server
//! copy, no backup and no sync — which means history is exactly as durable as
//! the phone it lives on, and the UI should not imply otherwise.
//!
//! Note the asymmetry that makes this worth having even though delivery is
//! live-only: **being unable to deliver a message does not mean forgetting it.**
//! Femi's outbox keeps what Femi wrote, marked undeliverable; Daniel's history
//! keeps what Daniel received. Neither device pretends the other's copy exists.

use std::collections::HashMap;

use super::{Conversation, DeliveryState, Message, MessageId};

/// Messages kept per conversation before the oldest are dropped.
///
/// A cap rather than unbounded growth, because this lives in memory on a phone
/// during a call. Persistent storage (`storage/`) is where a longer archive
/// belongs; this is the working set.
pub const MAX_PER_CONVERSATION: usize = 500;

/// Conversation history for this device.
#[derive(Debug, Default)]
pub struct History {
    conversations: HashMap<Conversation, Vec<Message>>,
}

impl History {
    /// Empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a message.
    ///
    /// Idempotent on message id: a message that arrives twice — a retransmit,
    /// or a room message reaching us both directly and via the relay — appears
    /// once.
    pub fn record(&mut self, message: Message) -> bool {
        let messages = self.conversations.entry(message.conversation).or_default();

        if messages.iter().any(|existing| existing.id == message.id) {
            return false;
        }

        messages.push(message);
        if messages.len() > MAX_PER_CONVERSATION {
            messages.remove(0);
        }
        true
    }

    /// Update an outbound message's delivery state.
    ///
    /// Returns false if the message is unknown, which is normal after the cap
    /// has dropped it and is not worth surfacing.
    pub fn update_delivery(&mut self, id: MessageId, delivery: DeliveryState) -> bool {
        for messages in self.conversations.values_mut() {
            if let Some(message) = messages.iter_mut().find(|m| m.id == id) {
                message.delivery = delivery;
                return true;
            }
        }
        false
    }

    /// Messages in a conversation, oldest first.
    #[must_use]
    pub fn conversation(&self, conversation: Conversation) -> &[Message] {
        self.conversations.get(&conversation).map_or(&[], Vec::as_slice)
    }

    /// The most recent message in a conversation, for a list preview.
    #[must_use]
    pub fn latest(&self, conversation: Conversation) -> Option<&Message> {
        self.conversations.get(&conversation)?.last()
    }

    /// Conversations that have any messages.
    #[must_use]
    pub fn conversations(&self) -> Vec<Conversation> {
        let mut keys: Vec<Conversation> = self.conversations.keys().copied().collect();
        // Most recent activity first — the order a chat list wants.
        keys.sort_by_key(|conversation| {
            core::cmp::Reverse(
                self.conversations[conversation].last().map_or(0, |m| m.at.as_millis()),
            )
        });
        keys
    }

    /// Drop one conversation.
    pub fn clear_conversation(&mut self, conversation: Conversation) {
        self.conversations.remove(&conversation);
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.conversations.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Monotonic;
    use crate::PeerId;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn message(body: &str, at: u64) -> Message {
        Message::compose(peer(1), Conversation::Direct(peer(2)), body, Monotonic(at)).unwrap()
    }

    #[test]
    fn messages_are_kept_in_order() {
        let mut history = History::new();
        history.record(message("first", 100));
        history.record(message("second", 200));

        let conversation = history.conversation(Conversation::Direct(peer(2)));
        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].body, "first");
        assert_eq!(history.latest(Conversation::Direct(peer(2))).unwrap().body, "second");
    }

    #[test]
    fn a_duplicate_message_is_recorded_once() {
        // A room message can reach us directly and via the relay.
        let mut history = History::new();
        let message = message("hello", 100);

        assert!(history.record(message.clone()));
        assert!(!history.record(message));

        assert_eq!(history.conversation(Conversation::Direct(peer(2))).len(), 1);
    }

    #[test]
    fn delivery_state_can_be_updated_after_sending() {
        let mut history = History::new();
        let message = message("hello", 100);
        let id = message.id;
        history.record(message);

        assert!(history.update_delivery(id, DeliveryState::Undeliverable));

        let stored = &history.conversation(Conversation::Direct(peer(2)))[0];
        assert_eq!(stored.delivery, DeliveryState::Undeliverable);
        assert!(stored.delivery.is_failure());
    }

    #[test]
    fn an_undeliverable_message_is_still_kept() {
        // No server to hold it does not mean forgetting it was written.
        let mut history = History::new();
        let message = message("are you around?", 100);
        let id = message.id;
        history.record(message);
        history.update_delivery(id, DeliveryState::Undeliverable);

        assert_eq!(history.conversation(Conversation::Direct(peer(2))).len(), 1);
    }

    #[test]
    fn history_is_capped_and_drops_the_oldest() {
        let mut history = History::new();
        for i in 0..(MAX_PER_CONVERSATION + 50) {
            history.record(message(&format!("m{i}"), i as u64));
        }

        let conversation = history.conversation(Conversation::Direct(peer(2)));
        assert_eq!(conversation.len(), MAX_PER_CONVERSATION);
        assert_eq!(conversation[0].body, "m50", "oldest should have been dropped");
    }

    #[test]
    fn conversations_are_listed_by_recent_activity() {
        let mut history = History::new();
        history.record(
            Message::compose(peer(1), Conversation::Direct(peer(2)), "old", Monotonic(100))
                .unwrap(),
        );
        history.record(
            Message::compose(peer(1), Conversation::Direct(peer(3)), "new", Monotonic(900))
                .unwrap(),
        );

        assert_eq!(history.conversations()[0], Conversation::Direct(peer(3)));
    }

    #[test]
    fn clearing_a_conversation_leaves_the_others() {
        let mut history = History::new();
        history.record(message("hello", 100));
        history.record(
            Message::compose(peer(1), Conversation::Direct(peer(3)), "hi", Monotonic(200))
                .unwrap(),
        );

        history.clear_conversation(Conversation::Direct(peer(2)));

        assert!(history.conversation(Conversation::Direct(peer(2))).is_empty());
        assert_eq!(history.conversation(Conversation::Direct(peer(3))).len(), 1);
    }
}
