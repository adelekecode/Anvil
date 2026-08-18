//! Wi-Fi Aware path (§14, §64).
//!
//! Wi-Fi Aware (NAN) gives Anvil peer-to-peer connectivity with no router, no
//! Internet and no pairing. It is also the single largest source of schedule
//! risk in this project, and the plan should say so plainly rather than
//! discovering it in Phase 4.
//!
//! ## What to expect
//!
//! **Availability is not universal.** Aware needs hardware support, OS support
//! *and* the feature to be currently enabled — location services on, Wi-Fi on,
//! sometimes more. Treat "no Aware on this device" as a normal state that
//! degrades to LAN-only, never as an error. [`crate::platform::Capabilities`]
//! is checked at startup and re-checked on every
//! [`crate::PlatformEvent::NetworkChanged`] for exactly this reason.
//!
//! **Cross-platform interop is the risk, not per-platform bring-up.** Android
//! and iOS each speak Aware; whether an Android publisher and an iOS subscriber
//! find each other, agree a data path, and carry IPv6 link-local UDP between
//! them is an empirical question. Phase 5 (§105) should be scheduled as an
//! investigation with a real possibility of a negative result, and the fallback
//! — one platform hosting a local group that the other joins as a LAN — should
//! be sketched before it is needed rather than after.
//!
//! **Addressing is IPv6 link-local, scoped to the Aware interface.** Addresses
//! are meaningless without their scope id, and they change between sessions.
//! This is precisely why [`crate::Endpoint`] is opaque to the protocol: the
//! core hands the string back to the adapter and never parses it.
//!
//! **Discovery has a duty cycle.** Peers are not found instantly, and the
//! discovery window is a power/latency trade-off the OS partly controls.
//! Discovery time is on the §93 measurement list; expect seconds, not
//! milliseconds, and design the join UX so that is not embarrassing.
//!
//! **It costs battery.** More than an established Wi-Fi association, which is
//! why [`PathKind::power_cost`] rates it higher and why LAN carries a small
//! static preference when scores are otherwise close.
//!
//! ## Aware and LAN together
//!
//! Both can be up at once, and often will be: same room, router present, Aware
//! also available. That is the good case — it is what makes §97's failover test
//! meaningful, and what lets the standby path exist at all. But a device may
//! not be able to hold an Aware data path and a Wi-Fi association on different
//! channels without the radio time-slicing, which shows up as jitter on both.
//! If measurements show that, the answer is a scoring input, not a special case:
//! the jitter is real and the existing metric already sees it.

use super::PathKind;

/// The transport family this module describes.
pub const KIND: PathKind = PathKind::WifiAware;

/// Service name published and subscribed over Aware.
///
/// Kept identical to the LAN service name so that a peer found on both looks
/// like one peer. Final correlation is cryptographic (§65), but matching
/// service names keep the pre-handshake UI honest.
pub const SERVICE_NAME: &str = crate::SERVICE_NAME;

/// Conservative datagram size for an Aware data path.
///
/// Lower than LAN: the effective MTU over an Aware NDP is smaller and less
/// predictable than Ethernet, and this is a floor to be raised by measurement,
/// not a target.
pub const CONSERVATIVE_DATAGRAM_SIZE: usize = 1_000;

/// Bytes of service-specific info an advertisement may carry.
///
/// Both platforms cap this hard — on the order of a couple of hundred bytes,
/// shared with everything else in the advertisement. Anvil's advertisement
/// payload must therefore stay tiny: an identity fingerprint and a room hint,
/// not a full identity key and certainly not a display name of arbitrary
/// length. See `protocol/discovery.md`.
pub const MAX_ADVERTISEMENT_BYTES: usize = 128;
