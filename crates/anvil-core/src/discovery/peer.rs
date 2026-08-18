//! Peer table and sighting de-duplication (§65).
//!
//! The problem this solves: Alice's phone is on the same Wi-Fi *and* in Wi-Fi
//! Aware range. Both discovery mechanisms find her. Without correlation the UI
//! shows "Alice" twice, the user has no idea which to tap, and the transport
//! layer treats her as two peers with one path each instead of one peer with
//! two paths — which quietly destroys the entire failover story.
//!
//! Correlation happens in two stages, and the distinction matters:
//!
//! 1. **Provisional**, on the advertised fingerprint. Cheap, immediate, and
//!    *not trustworthy* — anyone nearby can advertise anyone's fingerprint.
//!    Good enough to merge rows in a list.
//! 2. **Confirmed**, at handshake, when the peer proves possession of the
//!    private identity key. Only then does a sighting get a real
//!    [`PeerId`], and only then may the UI present the peer as who it claims
//!    to be.
//!
//! A peer that is provisionally correlated but not yet confirmed can therefore
//! be an impostor, and [`DiscoveredPeer::confirmed`] exists so the UI cannot
//! forget that.

use std::collections::HashMap;

use super::service::{Advertisement, Fingerprint};
use crate::time::Monotonic;
use crate::transport::{Endpoint, PathKind};
use crate::PeerId;

/// A raw sighting from one discovery mechanism.
#[derive(Clone, Debug)]
pub struct PeerAdvertisement {
    /// Which transport saw it.
    pub kind: PathKind,
    /// Adapter-local handle, used to match a later "lost" callback.
    pub handle: String,
    /// Where to reach it.
    pub endpoint: Endpoint,
    /// Decoded advertisement payload.
    pub advertisement: Advertisement,
}

/// A peer as the UI should see it: one person, however many radios found them.
#[derive(Clone, Debug)]
pub struct DiscoveredPeer {
    /// Provisional correlation key.
    pub fingerprint: Fingerprint,
    /// Confirmed identity, present only after a completed handshake.
    pub peer_id: Option<PeerId>,
    /// Advertised name. Unverified until `confirmed`.
    pub display_name: String,
    /// Advertised room, if hosting.
    pub room_hint: Option<u32>,
    /// Every transport this peer has been seen on, with the endpoint to use.
    pub endpoints: Vec<(PathKind, Endpoint)>,
    /// First sighting.
    pub first_seen: Monotonic,
    /// Most recent sighting on any transport.
    pub last_seen: Monotonic,
    /// Whether identity has been cryptographically confirmed.
    pub confirmed: bool,
}

impl DiscoveredPeer {
    /// Transports this peer is currently reachable on.
    #[must_use]
    pub fn kinds(&self) -> Vec<PathKind> {
        let mut kinds: Vec<_> = self.endpoints.iter().map(|(k, _)| *k).collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }

    /// Endpoint for a given transport.
    #[must_use]
    pub fn endpoint(&self, kind: PathKind) -> Option<&Endpoint> {
        self.endpoints.iter().find(|(k, _)| *k == kind).map(|(_, e)| e)
    }
}

/// What changed when a sighting was recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SightingOutcome {
    /// A peer nobody had seen before.
    New,
    /// A known peer, seen on a transport they were not previously on.
    PathAdded(PathKind),
    /// A known peer, same transport. Refreshes liveness only.
    Refreshed,
}

/// Everyone currently visible.
#[derive(Debug, Default)]
pub struct PeerTable {
    peers: HashMap<Fingerprint, DiscoveredPeer>,
}

impl PeerTable {
    /// Empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a sighting, merging it into an existing peer where possible.
    pub fn observe(&mut self, ad: &PeerAdvertisement, now: Monotonic) -> SightingOutcome {
        let fingerprint = ad.advertisement.fingerprint;

        let Some(peer) = self.peers.get_mut(&fingerprint) else {
            self.peers.insert(
                fingerprint,
                DiscoveredPeer {
                    fingerprint,
                    peer_id: None,
                    display_name: ad.advertisement.display_name.clone(),
                    room_hint: ad.advertisement.room_hint,
                    endpoints: vec![(ad.kind, ad.endpoint.clone())],
                    first_seen: now,
                    last_seen: now,
                    confirmed: false,
                },
            );
            return SightingOutcome::New;
        };

        peer.last_seen = now;
        peer.room_hint = ad.advertisement.room_hint;

        // Only accept a name change from an unconfirmed peer. Once identity is
        // proven, the name that came with the handshake stands — otherwise an
        // impostor could rename a confirmed peer out from under the user by
        // spamming advertisements.
        if !peer.confirmed {
            peer.display_name = ad.advertisement.display_name.clone();
        }

        match peer.endpoints.iter_mut().find(|(k, _)| *k == ad.kind) {
            Some(slot) => {
                slot.1 = ad.endpoint.clone(); // addresses move; keep the newest
                SightingOutcome::Refreshed
            }
            None => {
                peer.endpoints.push((ad.kind, ad.endpoint.clone()));
                SightingOutcome::PathAdded(ad.kind)
            }
        }
    }

    /// Promote a peer to confirmed after a successful handshake.
    ///
    /// Returns false if we have never seen an advertisement from this
    /// fingerprint — which is not an error. Wi-Fi Aware can hand over a data
    /// path before the subscriber has processed the matching advertisement, and
    /// an inbound connection from a peer we have not browsed yet is normal.
    pub fn confirm(&mut self, fingerprint: Fingerprint, peer_id: PeerId, name: String) -> bool {
        let Some(peer) = self.peers.get_mut(&fingerprint) else {
            return false;
        };
        peer.peer_id = Some(peer_id);
        peer.display_name = name;
        peer.confirmed = true;
        true
    }

    /// Drop one transport's endpoint for a peer.
    ///
    /// Returns `true` only if the peer has no endpoints left and is now gone
    /// entirely — losing LAN while Aware still works is not "peer lost", and
    /// emitting it as such would make the UI flicker every time a router
    /// hiccups.
    pub fn remove_endpoint(&mut self, fingerprint: Fingerprint, kind: PathKind) -> bool {
        let Some(peer) = self.peers.get_mut(&fingerprint) else {
            return false;
        };
        peer.endpoints.retain(|(k, _)| *k != kind);
        if peer.endpoints.is_empty() {
            self.peers.remove(&fingerprint);
            return true;
        }
        false
    }

    /// Expire peers not seen within `ttl`, returning those dropped.
    ///
    /// Necessary because discovery "lost" callbacks are unreliable on both
    /// platforms — a phone that walks out of range often just stops
    /// advertising, with no event at all.
    pub fn expire(&mut self, now: Monotonic, ttl: core::time::Duration) -> Vec<DiscoveredPeer> {
        let stale: Vec<Fingerprint> = self
            .peers
            .iter()
            .filter(|(_, p)| now.saturating_since(p.last_seen) > ttl)
            .map(|(fp, _)| *fp)
            .collect();

        stale.iter().filter_map(|fp| self.peers.remove(fp)).collect()
    }

    /// Look up by fingerprint.
    #[must_use]
    pub fn get(&self, fingerprint: &Fingerprint) -> Option<&DiscoveredPeer> {
        self.peers.get(fingerprint)
    }

    /// Look up by confirmed identity.
    #[must_use]
    pub fn by_peer_id(&self, peer_id: PeerId) -> Option<&DiscoveredPeer> {
        self.peers.values().find(|p| p.peer_id == Some(peer_id))
    }

    /// Everyone currently visible.
    pub fn iter(&self) -> impl Iterator<Item = &DiscoveredPeer> {
        self.peers.values()
    }

    /// How many peers are visible.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether nobody is visible.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;

    const FP: Fingerprint = [9, 9, 9, 9, 1, 2, 3, 4];

    fn sighting(kind: PathKind, address: &str, name: &str) -> PeerAdvertisement {
        PeerAdvertisement {
            kind,
            handle: format!("{kind}:{address}"),
            endpoint: Endpoint::new(kind, address),
            advertisement: Advertisement::new(FP, None, name),
        }
    }

    #[test]
    fn one_device_seen_on_two_transports_is_one_peer() {
        let mut table = PeerTable::new();

        assert_eq!(
            table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Alice"), Monotonic(100)),
            SightingOutcome::New
        );
        assert_eq!(
            table.observe(&sighting(PathKind::WifiAware, "aware:7", "Alice"), Monotonic(150)),
            SightingOutcome::PathAdded(PathKind::WifiAware)
        );

        assert_eq!(table.len(), 1, "the same phone appeared twice in the peer list");
        let peer = table.get(&FP).unwrap();
        assert_eq!(peer.kinds(), vec![PathKind::Lan, PathKind::WifiAware]);
    }

    #[test]
    fn repeat_sightings_refresh_rather_than_duplicate() {
        let mut table = PeerTable::new();
        table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Alice"), Monotonic(100));

        // Same transport, new address — a DHCP lease change, not a new peer.
        let outcome =
            table.observe(&sighting(PathKind::Lan, "10.0.0.9:47820", "Alice"), Monotonic(900));

        assert_eq!(outcome, SightingOutcome::Refreshed);
        assert_eq!(table.len(), 1);
        assert_eq!(
            table.get(&FP).unwrap().endpoint(PathKind::Lan).unwrap().address,
            "10.0.0.9:47820"
        );
        assert_eq!(table.get(&FP).unwrap().last_seen, Monotonic(900));
    }

    #[test]
    fn losing_one_transport_does_not_lose_the_peer() {
        let mut table = PeerTable::new();
        table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Alice"), Monotonic(100));
        table.observe(&sighting(PathKind::WifiAware, "aware:7", "Alice"), Monotonic(100));

        let gone = table.remove_endpoint(FP, PathKind::Lan);

        assert!(!gone, "peer reported lost while still reachable over Aware");
        assert_eq!(table.get(&FP).unwrap().kinds(), vec![PathKind::WifiAware]);

        assert!(table.remove_endpoint(FP, PathKind::WifiAware));
        assert!(table.is_empty());
    }

    #[test]
    fn peers_are_unconfirmed_until_they_prove_identity() {
        let mut table = PeerTable::new();
        table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Alice"), Monotonic(100));

        let peer = table.get(&FP).unwrap();
        assert!(!peer.confirmed);
        assert_eq!(peer.peer_id, None);

        let id = PeerId([7u8; 32]);
        assert!(table.confirm(FP, id, "Alice".into()));

        let peer = table.get(&FP).unwrap();
        assert!(peer.confirmed);
        assert_eq!(peer.peer_id, Some(id));
        assert_eq!(table.by_peer_id(id).map(|p| p.fingerprint), Some(FP));
    }

    #[test]
    fn a_confirmed_peer_cannot_be_renamed_by_an_advertisement() {
        let mut table = PeerTable::new();
        table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Alice"), Monotonic(100));
        table.confirm(FP, PeerId([7u8; 32]), "Alice".into());

        // A neighbour spams advertisements claiming Alice's fingerprint.
        table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Bank Support"), Monotonic(200));

        assert_eq!(table.get(&FP).unwrap().display_name, "Alice");
    }

    #[test]
    fn silent_peers_expire() {
        let mut table = PeerTable::new();
        table.observe(&sighting(PathKind::Lan, "10.0.0.5:47820", "Alice"), Monotonic(1_000));

        assert!(table.expire(Monotonic(5_000), Duration::from_secs(10)).is_empty());

        let dropped = table.expire(Monotonic(20_000), Duration::from_secs(10));
        assert_eq!(dropped.len(), 1);
        assert!(table.is_empty());
    }

    #[test]
    fn confirming_an_unseen_fingerprint_is_not_an_error() {
        // Inbound connection before the advertisement was processed.
        let mut table = PeerTable::new();
        assert!(!table.confirm(FP, PeerId([1u8; 32]), "Alice".into()));
    }
}
