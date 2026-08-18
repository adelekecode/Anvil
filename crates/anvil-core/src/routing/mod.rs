//! Turning room membership into packet destinations.
//!
//! Two steps, deliberately separate:
//!
//! ```text
//!   room state ──► Topology ──► Route (peer + path)
//!                  (who)        (how)
//! ```
//!
//! [`Topology`] answers "who should get this", from membership and the elected
//! relay. [`route`] answers "over which path", from the transport manager. A
//! relay change alters the first without touching the second; a Wi-Fi failover
//! alters the second without touching the first. Collapsing them into one step
//! would couple two things that must be able to fail independently.

mod route;
mod topology;

pub use route::{resolve_forward, resolve_media, Route};
pub use topology::Topology;
