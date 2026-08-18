//! The Anvil wire protocol.
//!
//! Two channels, chosen per packet type:
//!
//! ```text
//!   ControlMessage ──► QUIC reliable stream   (must arrive, in order)
//!   MediaPacket    ──► QUIC datagram          (must arrive soon, or not at all)
//! ```
//!
//! Everything here treats its input as hostile. Any device within radio range
//! can send bytes at these parsers with no prior relationship, so decode paths
//! check lengths before indexing, reject unknown values instead of guessing,
//! and never panic — including on deliberately malformed input, which is
//! asserted by test rather than assumed.

pub mod control;
pub mod header;
pub mod media;
pub mod packet;
pub mod version;

pub use control::ControlMessage;
pub use header::{MediaHeader, FLAG_RELAYED, FLAG_TALKSPURT_START, HEADER_LEN};
pub use media::{max_opus_payload, MediaPacket, TAG_LEN};
pub use packet::PacketType;
pub use version::{is_supported, negotiate, SUPPORTED};
