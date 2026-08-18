//! The elected local relay (§31–§41).
//!
//! Group rooms route media through one participant rather than a full mesh,
//! because mesh connections grow as N(N−1)/2 and phones have one radio (§32).
//!
//! The relay is **a room node doing a job**, not an authority and not a server.
//! It forwards sealed packets. It holds no media keys, makes no membership
//! decisions, and can be replaced without the room noticing anything beyond a
//! brief glitch (§33, §71). If the relay device happens to belong to a
//! participant — which it always does in v0.1 — that person can hear the room
//! because they are a *participant*, and the relay role adds nothing to what
//! they already had (§34).

mod election;
mod forwarding;
mod health;

pub use election::{elect, ElectionReason, ElectionResult, RelayCandidate};
pub use forwarding::{decide, resolve_sender, DropReason, ForwardDecision};
pub use health::{RelayHealth, RelayMonitor};
