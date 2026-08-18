//! Messages.
//!
//! Chat rides the peer session that already exists for voice, on a QUIC
//! **reliable stream** rather than a datagram. The distinction is the whole
//! reason the two can share a connection:
//!
//! ```text
//!   Anvil peer session
//!   ├── voice    QUIC datagram        late is worse than missing
//!   ├── chat     QUIC reliable stream must arrive, in order
//!   └── control  QUIC reliable stream must arrive, in order
//! ```
//!
//! ## The v0.1 scope decision: live messaging only
//!
//! If Daniel is not reachable, his message is not delivered. There is no server
//! to hold it, and inventing decentralised store-and-forward — which peer holds
//! it? for how long? who pays the battery? what stops it being an amplification
//! vector? — is a genuinely hard problem that would dominate the first release.
//!
//! So v0.1 is honest about it instead: a message either goes now or is marked
//! [`DeliveryState::Undeliverable`], and the UI says so plainly rather than
//! showing a hopeful clock icon forever. Store-and-forward can come later
//! without changing the message format.
//!
//! **History is local and permanent.** What is unavailable offline is
//! *delivery*, not the record of what was said.

use crate::time::Monotonic;
use crate::{PeerId, RoomId};

/// Longest message body accepted.
///
/// Bounded because it crosses the FFI boundary as JSON and lands in local
/// storage. Generous enough not to be felt in conversation.
pub const MAX_BODY: usize = 4_000;

/// Locally unique message identifier.
///
/// Random rather than sequential, so that two devices composing at the same
/// moment cannot collide, and so a message id leaks nothing about how much
/// anyone has said.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub [u8; 12]);

impl MessageId {
    /// Fresh identifier.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; 12];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
        Self(bytes)
    }

    /// Hex form, for the FFI boundary and logs.
    #[must_use]
    pub fn to_hex(self) -> String {
        use core::fmt::Write as _;
        let mut out = String::with_capacity(24);
        for byte in self.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

impl core::fmt::Debug for MessageId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MessageId({})", &self.to_hex()[..8])
    }
}

/// Who a message is addressed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Conversation {
    /// One peer.
    Direct(PeerId),
    /// Everyone in a room.
    Room(RoomId),
}

/// How far a message has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryState {
    /// Composed, not yet handed to the transport.
    Pending,
    /// Written to the peer's reliable stream.
    Sent,
    /// The peer acknowledged it at the application layer.
    ///
    /// Distinct from `Sent`: QUIC delivering bytes is not the same as the other
    /// app having them, and conflating the two is how "delivered" ticks start
    /// lying.
    Delivered,
    /// The recipient was not reachable. Not retried in v0.1 — see the module
    /// docs for why, and say so in the UI rather than implying it will arrive.
    Undeliverable,
    /// In a room, delivered to some members and not others.
    ///
    /// A room has no single delivery answer, and pretending otherwise would
    /// misrepresent what happened.
    Partial {
        /// How many members received it.
        delivered: u8,
        /// How many members were addressed.
        total: u8,
    },
}

impl DeliveryState {
    /// Whether the UI should show this as failed.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Undeliverable)
    }

    /// Whether delivery is still in progress.
    #[must_use]
    pub const fn is_in_flight(self) -> bool {
        matches!(self, Self::Pending | Self::Sent)
    }
}

/// One message, sent or received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// Identifier.
    pub id: MessageId,
    /// Who wrote it.
    pub from: PeerId,
    /// Where it belongs.
    pub conversation: Conversation,
    /// Body. Already length-checked.
    pub body: String,
    /// Local monotonic time it was composed or received.
    ///
    /// Deliberately not the sender's wall clock: with no internet there is no
    /// agreed time, and displaying a sender-controlled timestamp lets anyone
    /// place a message anywhere in your history.
    pub at: Monotonic,
    /// Delivery state. Only meaningful for outbound messages.
    pub delivery: DeliveryState,
    /// Whether this device wrote it.
    pub outbound: bool,
}

/// Why a message was refused before it was ever sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageError {
    /// Nothing but whitespace.
    Empty,
    /// Longer than [`MAX_BODY`].
    TooLong,
}

impl core::fmt::Display for MessageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Empty => "message is empty",
            Self::TooLong => "message is too long",
        })
    }
}

impl Message {
    /// Compose an outbound message.
    pub fn compose(
        from: PeerId,
        conversation: Conversation,
        body: &str,
        at: Monotonic,
    ) -> Result<Self, MessageError> {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return Err(MessageError::Empty);
        }
        if trimmed.len() > MAX_BODY {
            return Err(MessageError::TooLong);
        }

        Ok(Self {
            id: MessageId::generate(),
            from,
            conversation,
            body: trimmed.to_owned(),
            at,
            delivery: DeliveryState::Pending,
            outbound: true,
        })
    }

    /// Record a message that arrived.
    ///
    /// Truncates an over-long body rather than rejecting the message: the
    /// sender is authenticated, so this is a peer running a different version
    /// or a bug, and showing most of what someone said beats showing nothing.
    #[must_use]
    pub fn received(
        id: MessageId,
        from: PeerId,
        conversation: Conversation,
        body: &str,
        at: Monotonic,
    ) -> Self {
        let mut body = body.to_owned();
        if body.len() > MAX_BODY {
            let mut end = MAX_BODY;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            body.truncate(end);
        }

        Self {
            id,
            from,
            conversation,
            body,
            at,
            delivery: DeliveryState::Delivered,
            outbound: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    #[test]
    fn composing_trims_and_marks_pending() {
        let message =
            Message::compose(peer(1), Conversation::Direct(peer(2)), "  Hey  ", Monotonic(100))
                .unwrap();

        assert_eq!(message.body, "Hey");
        assert_eq!(message.delivery, DeliveryState::Pending);
        assert!(message.outbound);
        assert!(message.delivery.is_in_flight());
    }

    #[test]
    fn empty_messages_are_refused_before_the_network_sees_them() {
        for body in ["", "   ", "\n\t"] {
            assert_eq!(
                Message::compose(peer(1), Conversation::Direct(peer(2)), body, Monotonic::ZERO),
                Err(MessageError::Empty)
            );
        }
    }

    #[test]
    fn overlong_messages_are_refused() {
        let long = "x".repeat(MAX_BODY + 1);
        assert_eq!(
            Message::compose(peer(1), Conversation::Direct(peer(2)), &long, Monotonic::ZERO),
            Err(MessageError::TooLong)
        );
    }

    #[test]
    fn a_received_message_that_is_too_long_is_truncated_not_dropped() {
        let long = "é".repeat(MAX_BODY); // multibyte, so truncation must be careful
        let message = Message::received(
            MessageId::generate(),
            peer(2),
            Conversation::Direct(peer(1)),
            &long,
            Monotonic(50),
        );

        assert!(message.body.len() <= MAX_BODY);
        assert!(!message.body.is_empty(), "showing most of it beats showing none");
        assert!(!message.outbound);
    }

    #[test]
    fn undeliverable_is_a_failure_and_not_in_flight() {
        assert!(DeliveryState::Undeliverable.is_failure());
        assert!(!DeliveryState::Undeliverable.is_in_flight());
        assert!(!DeliveryState::Delivered.is_failure());
    }

    #[test]
    fn room_delivery_can_be_partial() {
        let state = DeliveryState::Partial { delivered: 2, total: 3 };
        assert!(!state.is_failure(), "reaching most of a room is not a failure");
        assert!(!state.is_in_flight());
    }

    #[test]
    fn message_ids_are_unique() {
        let ids: std::collections::HashSet<MessageId> =
            (0..1_000).map(|_| MessageId::generate()).collect();
        assert_eq!(ids.len(), 1_000);
    }

    #[test]
    fn direct_and_room_conversations_are_distinct() {
        let direct = Conversation::Direct(peer(2));
        let room = Conversation::Room(RoomId::generate());
        assert_ne!(format!("{direct:?}"), format!("{room:?}"));
    }
}
