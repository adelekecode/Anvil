//! Path ownership, selection and failover (§15, §16, §22, §23).
//!
//! The `TransportManager` is the only thing in Anvil that knows a path exists.
//! Above it, peers are reachable or not; below it, sockets come and go.
//!
//! It maintains, per peer:
//!
//! * every known candidate path,
//! * one **active** path carrying media,
//! * optionally one **standby** path, kept warm but *not* sent duplicate media.
//!
//! That last point is §23 and it is a real decision, not an omission. Duplicating
//! every frame over two radios doubles airtime, battery and CPU to buy
//! redundancy that failover already provides within a few hundred milliseconds.
//! Selective redundancy can come later, for the frames where it pays.

use std::collections::HashMap;

use super::{
    score_path, should_switch, Endpoint, Path, PathKind, PathSample, PathState, SwitchDecision,
};
use crate::config::TransportConfig;
use crate::time::Monotonic;
use crate::{PathId, PeerId};

/// Everything known about how to reach one peer.
#[derive(Clone, Debug)]
pub struct PeerConnection {
    /// Who.
    pub peer: PeerId,
    /// Every candidate path, keyed by id.
    pub paths: HashMap<PathId, Path>,
    /// Path currently carrying media.
    pub active: Option<PathId>,
    /// Warm alternative.
    pub standby: Option<PathId>,
    /// When the active path was adopted, for dwell-time enforcement.
    pub active_since: Monotonic,
}

impl PeerConnection {
    fn new(peer: PeerId, now: Monotonic) -> Self {
        Self { peer, paths: HashMap::new(), active: None, standby: None, active_since: now }
    }

    /// The active path, if any.
    #[must_use]
    pub fn active_path(&self) -> Option<&Path> {
        self.active.and_then(|id| self.paths.get(&id))
    }

    /// The standby path, if any.
    #[must_use]
    pub fn standby_path(&self) -> Option<&Path> {
        self.standby.and_then(|id| self.paths.get(&id))
    }

    /// Transports on which this peer is currently reachable, for
    /// [`crate::Event::TransportChanged`] and diagnostics.
    #[must_use]
    pub fn available_kinds(&self) -> Vec<PathKind> {
        let mut kinds: Vec<_> =
            self.paths.values().filter(|p| p.is_usable()).map(|p| p.kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }
}

/// What changed after an evaluation pass, so the engine knows what to announce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathChange {
    /// Peer affected.
    pub peer: PeerId,
    /// Path now carrying media.
    pub active: PathId,
    /// Path that was carrying it, if there was one.
    pub previous: Option<PathId>,
    /// Why the change happened.
    pub decision: SwitchDecision,
}

/// Owns all paths for all peers and decides which ones carry media.
#[derive(Debug)]
pub struct TransportManager {
    peers: HashMap<PeerId, PeerConnection>,
    config: TransportConfig,
    next_path_id: u64,
}

impl TransportManager {
    /// New manager.
    #[must_use]
    pub fn new(config: TransportConfig) -> Self {
        Self { peers: HashMap::new(), config, next_path_id: 1 }
    }

    /// Register a newly discovered way to reach a peer.
    ///
    /// Returns the assigned [`PathId`], which the caller hands to
    /// [`crate::platform::TransportAdapter::connect`]. Ids are allocated here,
    /// before the adapter has anything to name the path with, so a connection
    /// attempt is trackable from the moment it starts.
    pub fn add_candidate(
        &mut self,
        peer: PeerId,
        endpoint: Endpoint,
        hops: u8,
        now: Monotonic,
    ) -> PathId {
        let id = PathId(self.next_path_id);
        self.next_path_id += 1;

        let conn = self.peers.entry(peer).or_insert_with(|| PeerConnection::new(peer, now));
        conn.paths.insert(id, Path::candidate(id, peer, endpoint, hops, now));
        id
    }

    /// Mark a path ready once the adapter reports it established.
    pub fn on_established(&mut self, path: PathId, max_datagram_size: usize, now: Monotonic) {
        if let Some(p) = self.path_mut(path) {
            p.state = PathState::Ready;
            p.max_datagram_size = Some(max_datagram_size);
            p.metrics.last_activity = now;
        }
    }

    /// Fold a measurement into a path.
    pub fn on_sample(&mut self, path: PathId, sample: PathSample, now: Monotonic) {
        if let Some(p) = self.path_mut(path) {
            p.metrics.observe(sample, now);
        }
    }

    /// Handle hard path loss.
    ///
    /// Returns the peer whose active path just died, if any, so the engine can
    /// evaluate immediately rather than waiting for the next tick — every
    /// millisecond here is silence the user hears.
    pub fn on_lost(&mut self, path: PathId, now: Monotonic) -> Option<PeerId> {
        let peer = self.path_mut(path).map(|p| {
            p.state = PathState::Failed;
            p.metrics.observe(PathSample::Disruption, now);
            p.peer
        })?;

        let conn = self.peers.get_mut(&peer)?;
        if conn.standby == Some(path) {
            conn.standby = None;
        }
        if conn.active == Some(path) {
            conn.active = None;
            return Some(peer);
        }
        None
    }

    /// Re-evaluate every peer and return the changes made.
    ///
    /// Called on a tick and whenever something happens that could change the
    /// answer. Idempotent: calling it twice with no new information makes no
    /// second change, because dwell time and hysteresis are both functions of
    /// state rather than of call count.
    pub fn evaluate(&mut self, now: Monotonic) -> Vec<PathChange> {
        let peers: Vec<PeerId> = self.peers.keys().copied().collect();
        peers.iter().filter_map(|peer| self.evaluate_peer(*peer, now)).collect()
    }

    fn evaluate_peer(&mut self, peer: PeerId, now: Monotonic) -> Option<PathChange> {
        let config = self.config;
        let conn = self.peers.get_mut(&peer)?;

        // Rank usable paths best-first.
        let mut ranked: Vec<(PathId, f32)> = conn
            .paths
            .values()
            .filter(|p| p.is_usable())
            .map(|p| (p.id, score_path(&p.metrics, p.kind, &config, now)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

        let (best_id, best_score) = *ranked.first()?;

        // Keep the runner-up warm, if we are keeping one at all.
        conn.standby =
            if config.maintain_standby { ranked.get(1).map(|(id, _)| *id) } else { None };

        let Some(active_id) = conn.active else {
            // Nothing active: adopt the best path. This is the cold-start and
            // post-failure case, and it is not a "switch" — there was nothing
            // to switch away from, so hysteresis has no meaning here.
            conn.active = Some(best_id);
            conn.active_since = now;
            return Some(PathChange {
                peer,
                active: best_id,
                previous: None,
                decision: SwitchDecision::Failover,
            });
        };

        if best_id == active_id {
            return None;
        }

        let active = conn.paths.get(&active_id)?;
        let active_score = score_path(&active.metrics, active.kind, &config, now);
        let decision = should_switch(
            active_score,
            &active.metrics,
            conn.active_since,
            best_score,
            &config,
            now,
        );

        match decision {
            SwitchDecision::Stay => None,
            SwitchDecision::Switch | SwitchDecision::Failover => {
                conn.active = Some(best_id);
                conn.active_since = now;
                if conn.standby == Some(best_id) {
                    conn.standby = Some(active_id);
                }
                Some(PathChange { peer, active: best_id, previous: Some(active_id), decision })
            }
        }
    }

    /// The path media should currently be sent on.
    #[must_use]
    pub fn active_path(&self, peer: PeerId) -> Option<&Path> {
        self.peers.get(&peer)?.active_path()
    }

    /// Everything known about a peer's connectivity.
    #[must_use]
    pub fn connection(&self, peer: PeerId) -> Option<&PeerConnection> {
        self.peers.get(&peer)
    }

    /// Every peer with at least one usable path.
    pub fn reachable_peers(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers.values().filter(|c| c.active.is_some()).map(|c| c.peer)
    }

    /// Forget a peer entirely — they left the room or vanished.
    pub fn remove_peer(&mut self, peer: PeerId) -> Option<PeerConnection> {
        self.peers.remove(&peer)
    }

    fn path_mut(&mut self, id: PathId) -> Option<&mut Path> {
        self.peers.values_mut().find_map(|c| c.paths.get_mut(&id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    /// Bring a path up and feed it enough good measurements to be trusted.
    fn establish(mgr: &mut TransportManager, id: PathId, rtt_ms: u64, now: Monotonic) {
        mgr.on_established(id, 1200, now);
        mgr.on_sample(id, PathSample::Rtt(Duration::from_millis(rtt_ms)), now);
        mgr.on_sample(id, PathSample::Delivery { expected: 100, received: 100 }, now);
    }

    #[test]
    fn adopts_the_first_usable_path() {
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);
        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );

        // Nothing is usable until the adapter says the path is up.
        assert!(mgr.evaluate(Monotonic(100)).is_empty());

        establish(&mut mgr, lan, 4, Monotonic(200));
        let changes = mgr.evaluate(Monotonic(200));

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].active, lan);
        assert_eq!(changes[0].previous, None);
        assert_eq!(mgr.active_path(alice).map(|p| p.id), Some(lan));
    }

    #[test]
    fn second_path_becomes_standby_not_active() {
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);
        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );
        establish(&mut mgr, lan, 4, Monotonic(100));
        mgr.evaluate(Monotonic(100));

        let aware = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::WifiAware, "aware:1"),
            0,
            Monotonic(200),
        );
        establish(&mut mgr, aware, 6, Monotonic(200));
        let changes = mgr.evaluate(Monotonic(60_000));

        assert!(changes.is_empty(), "comparable paths must not cause a switch");
        let conn = mgr.connection(alice).unwrap();
        assert_eq!(conn.active, Some(lan));
        assert_eq!(conn.standby, Some(aware));
        assert_eq!(conn.available_kinds(), vec![PathKind::Lan, PathKind::WifiAware]);
    }

    #[test]
    fn losing_the_active_path_fails_over_to_standby_immediately() {
        // This is the §97 adaptive transport test, in miniature: LAN active,
        // Aware standby, router unplugged.
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);

        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );
        let aware = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::WifiAware, "aware:1"),
            0,
            Monotonic::ZERO,
        );
        establish(&mut mgr, lan, 4, Monotonic(100));
        establish(&mut mgr, aware, 8, Monotonic(100));
        mgr.evaluate(Monotonic(100));
        assert_eq!(mgr.connection(alice).unwrap().active, Some(lan));

        // Router disappears one second in — well inside min_dwell.
        let affected = mgr.on_lost(lan, Monotonic(1_100));
        assert_eq!(affected, Some(alice));

        let changes = mgr.evaluate(Monotonic(1_100));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].active, aware);
        assert_eq!(changes[0].decision, SwitchDecision::Failover);
        assert_eq!(mgr.active_path(alice).map(|p| p.kind), Some(PathKind::WifiAware));
    }

    #[test]
    fn losing_a_standby_path_does_not_disturb_the_call() {
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);
        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );
        let aware = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::WifiAware, "aware:1"),
            0,
            Monotonic::ZERO,
        );
        establish(&mut mgr, lan, 4, Monotonic(100));
        establish(&mut mgr, aware, 8, Monotonic(100));
        mgr.evaluate(Monotonic(100));

        assert_eq!(mgr.on_lost(aware, Monotonic(2_000)), None);
        assert_eq!(mgr.connection(alice).unwrap().active, Some(lan));
        assert_eq!(mgr.connection(alice).unwrap().standby, None);
    }

    #[test]
    fn degraded_active_path_yields_to_a_clean_one_after_dwell() {
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);
        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );
        let aware = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::WifiAware, "aware:1"),
            0,
            Monotonic::ZERO,
        );
        establish(&mut mgr, lan, 4, Monotonic(100));
        establish(&mut mgr, aware, 5, Monotonic(100));
        mgr.evaluate(Monotonic(100));
        assert_eq!(mgr.connection(alice).unwrap().active, Some(lan));

        // LAN rots: heavy loss and latency, still technically alive.
        for t in (2_000..30_000).step_by(1_000) {
            let now = Monotonic(t);
            mgr.on_sample(lan, PathSample::Rtt(Duration::from_millis(180)), now);
            mgr.on_sample(lan, PathSample::Delivery { expected: 100, received: 82 }, now);
            mgr.on_sample(aware, PathSample::Rtt(Duration::from_millis(6)), now);
            mgr.on_sample(aware, PathSample::Delivery { expected: 100, received: 100 }, now);
        }

        let changes = mgr.evaluate(Monotonic(30_000));
        assert_eq!(changes.len(), 1, "expected a voluntary switch");
        assert_eq!(changes[0].active, aware);
        assert_eq!(changes[0].decision, SwitchDecision::Switch);
    }

    #[test]
    fn evaluation_is_idempotent() {
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);
        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );
        establish(&mut mgr, lan, 4, Monotonic(100));

        assert_eq!(mgr.evaluate(Monotonic(100)).len(), 1);
        assert!(mgr.evaluate(Monotonic(100)).is_empty());
        assert!(mgr.evaluate(Monotonic(200)).is_empty());
    }

    #[test]
    fn peers_with_no_usable_path_are_not_reachable() {
        let mut mgr = TransportManager::new(TransportConfig::default());
        let alice = peer(1);
        let lan = mgr.add_candidate(
            alice,
            Endpoint::new(PathKind::Lan, "10.0.0.5:7000"),
            0,
            Monotonic::ZERO,
        );
        establish(&mut mgr, lan, 4, Monotonic(100));
        mgr.evaluate(Monotonic(100));
        assert_eq!(mgr.reachable_peers().count(), 1);

        mgr.on_lost(lan, Monotonic(2_000));
        mgr.evaluate(Monotonic(2_000));
        assert_eq!(mgr.reachable_peers().count(), 0);
    }
}
