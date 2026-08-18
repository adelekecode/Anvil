//! One candidate path to one peer.

use super::{Endpoint, PathKind, PathMetrics};
use crate::time::Monotonic;
use crate::{PathId, PeerId};

/// Lifecycle of a path.
///
/// `Failed` is kept in the table rather than deleted so that a path which keeps
/// dying is visibly a repeat offender: [`PathMetrics::disruptions`] survives,
/// and stability scoring can hold it against the path on the next attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathState {
    /// Discovered but not yet connected.
    Candidate,
    /// Connection in progress.
    Connecting,
    /// Usable.
    Ready,
    /// Was ready, has failed. Eligible for re-establishment.
    Failed,
    /// Deliberately torn down.
    Closed,
}

impl PathState {
    /// Whether traffic can be sent right now.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// A path to a peer over one transport.
#[derive(Clone, Debug)]
pub struct Path {
    /// Local identifier. Unique for the lifetime of the process; a
    /// re-established path gets a fresh one so stale metrics can never be
    /// attributed to a new connection.
    pub id: PathId,
    /// Peer at the far end.
    pub peer: PeerId,
    /// Transport family.
    pub kind: PathKind,
    /// Where to reach the peer.
    pub endpoint: Endpoint,
    /// Current state.
    pub state: PathState,
    /// Measurements.
    pub metrics: PathMetrics,
    /// Largest datagram this path carries unfragmented, once known.
    ///
    /// Set from [`crate::PlatformEvent::PathEstablished`]. Fragmenting an Opus
    /// frame across datagrams would mean one lost fragment destroys a frame
    /// that was otherwise recoverable, so media is sized to fit this.
    pub max_datagram_size: Option<usize>,
}

impl Path {
    /// A newly discovered candidate.
    #[must_use]
    pub fn candidate(id: PathId, peer: PeerId, endpoint: Endpoint, hops: u8, now: Monotonic)
        -> Self
    {
        Self {
            id,
            peer,
            kind: endpoint.kind,
            endpoint,
            state: PathState::Candidate,
            metrics: PathMetrics::new(now, hops),
            max_datagram_size: None,
        }
    }

    /// Whether traffic can be sent on this path right now.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.state.is_usable()
    }
}
