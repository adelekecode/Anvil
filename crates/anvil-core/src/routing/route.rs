//! Where a packet goes next.
//!
//! A thin layer that turns "who should receive this" (topology) into "which
//! path carries it" (transport). Keeping it separate means a relay change and a
//! path change are independent events — which is what lets Wi-Fi fail over
//! without disturbing the relay, and the relay change without disturbing the
//! paths.

use crate::transport::TransportManager;
use crate::{PathId, PeerId};

use super::Topology;

/// A resolved delivery: who, over which path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    /// Recipient.
    pub peer: PeerId,
    /// Path to use.
    pub path: PathId,
}

/// Resolve outgoing media to concrete paths.
///
/// Returns an empty vector when nothing is deliverable, which is a normal
/// transient state during an election or a path switch — not an error, and not
/// something to tear the room down over.
#[must_use]
pub fn resolve_media(
    topology: &Topology,
    members: &[PeerId],
    local: PeerId,
    transport: &TransportManager,
) -> Vec<Route> {
    match topology {
        Topology::Pending => Vec::new(),

        Topology::Direct { peer } => transport
            .active_path(*peer)
            .map(|path| vec![Route { peer: *peer, path: path.id }])
            .unwrap_or_default(),

        Topology::Relayed { relay, is_local: false } => transport
            .active_path(*relay)
            .map(|path| vec![Route { peer: *relay, path: path.id }])
            .unwrap_or_default(),

        // We are the relay: fan our own media out directly, since routing it
        // through ourselves would be absurd.
        Topology::Relayed { is_local: true, .. } => members
            .iter()
            .filter(|m| **m != local)
            .filter_map(|m| {
                transport.active_path(*m).map(|path| Route { peer: *m, path: path.id })
            })
            .collect(),
    }
}

/// Resolve a relay's fan-out list to paths.
///
/// Members with no active path are silently skipped: one unreachable
/// participant must not stop the packet reaching everyone else.
#[must_use]
pub fn resolve_forward(recipients: &[PeerId], transport: &TransportManager) -> Vec<Route> {
    recipients
        .iter()
        .filter_map(|peer| {
            transport.active_path(*peer).map(|path| Route { peer: *peer, path: path.id })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Monotonic;
    use crate::transport::{Endpoint, PathKind, PathSample, TransportManager};
    use crate::TransportConfig;
    use core::time::Duration;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn manager_with(peers: &[PeerId]) -> TransportManager {
        let mut mgr = TransportManager::new(TransportConfig::default());
        for p in peers {
            let id = mgr.add_candidate(*p, Endpoint::new(PathKind::Lan, "10.0.0.1:47820"), 0,
                                       Monotonic::ZERO);
            mgr.on_established(id, 1_200, Monotonic(100));
            mgr.on_sample(id, PathSample::Rtt(Duration::from_millis(4)), Monotonic(100));
        }
        mgr.evaluate(Monotonic(100));
        mgr
    }

    #[test]
    fn direct_media_goes_to_the_one_peer() {
        let mgr = manager_with(&[peer(2)]);
        let routes = resolve_media(&Topology::Direct { peer: peer(2) }, &[peer(1), peer(2)],
                                   peer(1), &mgr);

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].peer, peer(2));
    }

    #[test]
    fn relayed_media_goes_only_to_the_relay() {
        // The §74 upload saving: one copy, not three.
        let mgr = manager_with(&[peer(2), peer(3), peer(4)]);
        let topology = Topology::Relayed { relay: peer(2), is_local: false };
        let members = [peer(1), peer(2), peer(3), peer(4)];

        let routes = resolve_media(&topology, &members, peer(1), &mgr);

        assert_eq!(routes.len(), 1, "a sender must upload one stream, not N-1");
        assert_eq!(routes[0].peer, peer(2));
    }

    #[test]
    fn the_relay_fans_its_own_media_out_directly() {
        let mgr = manager_with(&[peer(1), peer(3), peer(4)]);
        let topology = Topology::Relayed { relay: peer(2), is_local: true };
        let members = [peer(1), peer(2), peer(3), peer(4)];

        let routes = resolve_media(&topology, &members, peer(2), &mgr);

        assert_eq!(routes.len(), 3);
        assert!(!routes.iter().any(|r| r.peer == peer(2)));
    }

    #[test]
    fn nothing_is_routed_while_pending() {
        let mgr = manager_with(&[peer(2)]);
        assert!(resolve_media(&Topology::Pending, &[peer(1), peer(2)], peer(1), &mgr).is_empty());
    }

    #[test]
    fn an_unreachable_peer_is_skipped_not_fatal() {
        // peer(4) has no path at all.
        let mgr = manager_with(&[peer(2), peer(3)]);
        let routes = resolve_forward(&[peer(2), peer(3), peer(4)], &mgr);

        assert_eq!(routes.len(), 2);
        assert!(!routes.iter().any(|r| r.peer == peer(4)));
    }

    #[test]
    fn a_peer_with_no_path_yields_no_route_rather_than_an_error() {
        let mgr = TransportManager::new(TransportConfig::default());
        assert!(resolve_media(&Topology::Direct { peer: peer(2) }, &[], peer(1), &mgr).is_empty());
    }
}
