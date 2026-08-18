//! Text messaging over the existing peer session.
//!
//! Chat needed no new transport, no new session and no new trust model. The
//! authenticated peer session built for voice already carries everything
//! required; chat is another channel on it.
//!
//! ```text
//!   Anvil peer session
//!   ├── voice    QUIC datagram         real-time, lossy
//!   ├── chat     QUIC reliable stream  must arrive, in order
//!   └── control  QUIC reliable stream  must arrive, in order
//! ```
//!
//! That is the payoff from choosing QUIC: two delivery semantics over one
//! connection, one handshake, one key agreement, one path-failover story.
//!
//! Room chat uses the same relay fan-out as room voice, and the same per-sender
//! keys — the relay forwards sealed bytes and cannot read messages any more
//! than it can hear speech.
//!
//! ## v0.1 scope
//!
//! Live delivery only. See [`message`] for why store-and-forward is deferred
//! and what the UI must say instead of pretending.

mod history;
mod message;

pub use history::{History, MAX_PER_CONVERSATION};
pub use message::{
    Conversation, DeliveryState, Message, MessageError, MessageId, MAX_BODY,
};
