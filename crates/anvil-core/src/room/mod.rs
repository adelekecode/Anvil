//! Rooms: creation, membership, lifecycle (§66–§71).
//!
//! A room is a set of authenticated participants sharing a key epoch. It is
//! **not** a connection, a relay, or a network. It survives all three changing,
//! which is the property everything else in this crate is arranged to protect.
//!
//! State is distributed — every participant holds their own copy and they
//! converge through authenticated control messages. There is no authoritative
//! node, including the relay (§71).

mod events;
mod join_code;
mod membership;
mod state;

pub use events::RoomTransition;
pub use join_code::{JoinCode, RoomIdentity};
pub use membership::{AdmissionPolicy, Participant};
pub use state::{RoomSnapshot, RoomState};
