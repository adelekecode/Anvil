//! Finding nearby devices (§62–§65).
//!
//! Two mechanisms, one result:
//!
//! ```text
//!   LAN discovery  ──┐
//!                    ├──►  PeerTable  ──►  DiscoveredPeer (one per person)
//!   Aware discovery ─┘
//! ```
//!
//! Discovery deliberately does *not* use the Anvil packet protocol. It rides
//! the platform's own mechanisms — NSD/Bonjour TXT records and Wi-Fi Aware
//! service info — because those are what work with no router, no DNS and no
//! prior contact between devices. The payload is a tiny opaque blob
//! ([`Advertisement`]) that the adapters copy around without understanding.
//!
//! Nothing discovered here is trusted. See [`peer`] for the two-stage
//! correlation model and why the distinction is load-bearing.

mod peer;
mod service;

pub use peer::{DiscoveredPeer, PeerAdvertisement, PeerTable, SightingOutcome};
pub use service::{Advertisement, Fingerprint, FINGERPRINT_LEN};

use core::time::Duration;

/// How long a peer survives without a sighting before being dropped.
///
/// Generous relative to the advertisement interval because both platforms lose
/// "peer went away" callbacks, and because Wi-Fi Aware discovery is duty-cycled
/// — a peer can genuinely go quiet for several seconds while still being right
/// there. Dropping them from the list and adding them back is a worse
/// experience than a stale row.
pub const PEER_TTL: Duration = Duration::from_secs(30);

/// How often to refresh this node's own advertisement.
pub const ADVERTISE_INTERVAL: Duration = Duration::from_secs(5);
