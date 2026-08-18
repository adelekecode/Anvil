//! Known peers and trust-on-first-use.
//!
//! Anvil has no certificate authority and no directory, so the only thing it can
//! honestly tell a user is: *is this the same device you talked to before?*
//! That is SSH's model, and it is the right one here for the same reason — the
//! alternative would be inventing an authority Anvil deliberately does not have.
//!
//! ```text
//!   first meeting   →  record the key.        "Daniel — new"
//!   same key again  →  recognise it.          "Daniel ★"
//!   same name, new key →  WARN. LOUDLY.       "Daniel's identity has changed"
//! ```
//!
//! ## The attack this defends against
//!
//! Display names are free. Anyone in radio range can advertise "Daniel". Without
//! this, a stranger typing the right name is indistinguishable from the person
//! you spoke to yesterday.
//!
//! With it, the impostor is a **new peer who happens to share a name** — which
//! the UI can show plainly — and the far more dangerous case, someone
//! substituting a new key for a name you already trust, produces an explicit
//! warning rather than silence.
//!
//! ## What TOFU does not defend against
//!
//! The *first* meeting. If the very first "Daniel" you meet is an impostor, TOFU
//! faithfully remembers the impostor. That is inherent to the model, and it is
//! why out-of-band verification (QR, in person) exists as a second step: it
//! upgrades a peer from "same as last time" to "confirmed by a human".
//!
//! The honest summary for the UI: **unverified means unverified**, not "probably
//! fine".

use std::collections::HashMap;

use crate::time::Monotonic;
use crate::PeerId;

use super::Fingerprint;

/// How much a user has confirmed about a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustState {
    /// Seen and remembered, never confirmed out of band.
    ///
    /// The normal state for almost everyone. Not a warning — just not a promise.
    Unverified,

    /// Confirmed out of band: QR scan, or comparing fingerprints in person.
    Verified,

    /// A peer claiming a known name presented a **different key**.
    ///
    /// The dangerous state. Either the person reinstalled Anvil — which
    /// genuinely regenerates their identity — or someone is impersonating them.
    /// Anvil cannot tell which, and must not guess.
    Changed,
}

impl TrustState {
    /// Whether the UI should show a warning.
    #[must_use]
    pub const fn needs_warning(self) -> bool {
        matches!(self, Self::Changed)
    }
}

/// A peer this device has met before. Stored locally, and only locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownPeer {
    /// Cryptographic identity.
    pub peer_id: PeerId,
    /// Public identity key, as first recorded.
    pub public_key: [u8; 32],
    /// Most recent display name they advertised.
    pub display_name: String,
    /// First time we met.
    pub first_seen: Monotonic,
    /// Most recent contact.
    pub last_seen: Monotonic,
    /// Trust state.
    pub trust: TrustState,
    /// Fingerprint of the key we previously trusted, when trust is
    /// [`TrustState::Changed`], so the UI can show what changed rather than
    /// just that something did.
    pub previous_fingerprint: Option<Fingerprint>,
}

impl KnownPeer {
    /// Fingerprint of the current key.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(self.peer_id)
    }
}

/// What happened when a peer presented an identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TofuOutcome {
    /// Never met before. Recorded.
    FirstContact,

    /// Same key, same name.
    Recognised,

    /// Same key, different name. Not suspicious on its own — people rename
    /// themselves — but worth showing, because a *sudden* rename to match
    /// someone else is a social attack even when the crypto is honest.
    Renamed {
        /// What we knew them as.
        previous_name: String,
    },

    /// **Same name, different key.** The warning case.
    IdentityChanged {
        /// Fingerprint we previously trusted for this name.
        previous_fingerprint: Fingerprint,
        /// Fingerprint now being presented.
        new_fingerprint: Fingerprint,
    },
}

impl TofuOutcome {
    /// Whether this should be surfaced to the user rather than logged.
    #[must_use]
    pub const fn is_noteworthy(&self) -> bool {
        matches!(self, Self::IdentityChanged { .. } | Self::Renamed { .. })
    }
}

/// Everyone this device has met. On-device only; never synced anywhere.
#[derive(Debug, Default)]
pub struct KnownPeers {
    peers: HashMap<PeerId, KnownPeer>,
}

impl KnownPeers {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a peer that has just completed an authenticated handshake.
    ///
    /// Only call this **after** the peer has proved possession of the private
    /// key. Recording an unauthenticated claim would let anyone poison the
    /// store by advertising, which would turn the identity-change warning from
    /// a defence into a nuisance that trains users to dismiss it.
    pub fn observe(
        &mut self,
        peer_id: PeerId,
        public_key: [u8; 32],
        display_name: &str,
        now: Monotonic,
    ) -> TofuOutcome {
        // Same key: a peer we know. Keys are identities, so this is the
        // authoritative match.
        if let Some(known) = self.peers.get_mut(&peer_id) {
            known.last_seen = now;

            if known.display_name != display_name {
                let previous_name = core::mem::replace(
                    &mut known.display_name,
                    display_name.to_owned(),
                );
                return TofuOutcome::Renamed { previous_name };
            }
            return TofuOutcome::Recognised;
        }

        // Different key. Do we already trust *this name* with another key?
        let collision = self
            .peers
            .values()
            .find(|known| known.display_name == display_name)
            .map(|known| (known.peer_id, known.fingerprint()));

        let new_fingerprint = Fingerprint::of(peer_id);

        let (trust, previous_fingerprint, outcome) = match collision {
            Some((_, previous_fingerprint)) => (
                TrustState::Changed,
                Some(previous_fingerprint),
                TofuOutcome::IdentityChanged { previous_fingerprint, new_fingerprint },
            ),
            None => (TrustState::Unverified, None, TofuOutcome::FirstContact),
        };

        self.peers.insert(
            peer_id,
            KnownPeer {
                peer_id,
                public_key,
                display_name: display_name.to_owned(),
                first_seen: now,
                last_seen: now,
                trust,
                previous_fingerprint,
            },
        );

        outcome
    }

    /// Mark a peer verified after an out-of-band check.
    ///
    /// Clears any previous change warning: the user has looked at the new key
    /// and accepted it, which is exactly the human judgement the warning was
    /// asking for.
    pub fn mark_verified(&mut self, peer_id: PeerId) -> bool {
        let Some(known) = self.peers.get_mut(&peer_id) else {
            return false;
        };
        known.trust = TrustState::Verified;
        known.previous_fingerprint = None;
        true
    }

    /// Accept a changed identity without verifying it — "yes, they reinstalled".
    ///
    /// Downgrades to [`TrustState::Unverified`] rather than up to `Verified`,
    /// because dismissing a warning is not the same as checking a fingerprint
    /// and should not be recorded as though it were.
    pub fn accept_change(&mut self, peer_id: PeerId) -> bool {
        let Some(known) = self.peers.get_mut(&peer_id) else {
            return false;
        };
        known.trust = TrustState::Unverified;
        known.previous_fingerprint = None;
        true
    }

    /// Forget a peer entirely.
    pub fn forget(&mut self, peer_id: PeerId) -> Option<KnownPeer> {
        self.peers.remove(&peer_id)
    }

    /// Look one up.
    #[must_use]
    pub fn get(&self, peer_id: PeerId) -> Option<&KnownPeer> {
        self.peers.get(&peer_id)
    }

    /// Whether this peer has been met before.
    #[must_use]
    pub fn is_known(&self, peer_id: PeerId) -> bool {
        self.peers.contains_key(&peer_id)
    }

    /// Everyone known, most recently seen first.
    #[must_use]
    pub fn all(&self) -> Vec<&KnownPeer> {
        let mut peers: Vec<&KnownPeer> = self.peers.values().collect();
        peers.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.peer_id.cmp(&b.peer_id)));
        peers
    }

    /// How many peers are known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether nobody is known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn key(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn first_contact_is_recorded_as_unverified() {
        let mut known = KnownPeers::new();

        let outcome = known.observe(peer(1), key(1), "Daniel", Monotonic(100));

        assert_eq!(outcome, TofuOutcome::FirstContact);
        assert!(!outcome.is_noteworthy(), "meeting someone new is not an alarm");
        assert_eq!(known.get(peer(1)).unwrap().trust, TrustState::Unverified);
    }

    #[test]
    fn meeting_the_same_peer_again_is_recognised() {
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));

        let outcome = known.observe(peer(1), key(1), "Daniel", Monotonic(9_000));

        assert_eq!(outcome, TofuOutcome::Recognised);
        assert_eq!(known.get(peer(1)).unwrap().last_seen, Monotonic(9_000));
        assert_eq!(known.len(), 1);
    }

    #[test]
    fn the_same_name_with_a_different_key_raises_the_warning() {
        // The attack: someone advertises a name you already trust.
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));

        let outcome = known.observe(peer(2), key(2), "Daniel", Monotonic(200));

        match outcome {
            TofuOutcome::IdentityChanged { previous_fingerprint, new_fingerprint } => {
                assert_ne!(previous_fingerprint, new_fingerprint);
            }
            other => panic!("impostor went unflagged: {other:?}"),
        }
        assert!(outcome.is_noteworthy());
        assert_eq!(known.get(peer(2)).unwrap().trust, TrustState::Changed);
        assert!(known.get(peer(2)).unwrap().trust.needs_warning());
    }

    #[test]
    fn the_original_peer_is_not_disturbed_by_an_impostor() {
        // An impostor must not be able to degrade the real peer's standing —
        // otherwise the attack becomes a denial of trust.
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));
        known.mark_verified(peer(1));

        known.observe(peer(2), key(2), "Daniel", Monotonic(200));

        assert_eq!(known.get(peer(1)).unwrap().trust, TrustState::Verified);
    }

    #[test]
    fn two_different_people_with_the_same_key_is_impossible_by_construction() {
        // Identity is the key. Same key, new name = a rename, not a new person.
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));

        let outcome = known.observe(peer(1), key(1), "Dan", Monotonic(200));

        assert_eq!(outcome, TofuOutcome::Renamed { previous_name: "Daniel".into() });
        assert_eq!(known.len(), 1);
        assert_eq!(known.get(peer(1)).unwrap().display_name, "Dan");
    }

    #[test]
    fn verification_clears_a_change_warning() {
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));
        known.observe(peer(2), key(2), "Daniel", Monotonic(200));
        assert!(known.get(peer(2)).unwrap().trust.needs_warning());

        assert!(known.mark_verified(peer(2)));

        let peer2 = known.get(peer(2)).unwrap();
        assert_eq!(peer2.trust, TrustState::Verified);
        assert_eq!(peer2.previous_fingerprint, None);
    }

    #[test]
    fn dismissing_a_warning_does_not_count_as_verifying() {
        // "They reinstalled" is a plausible explanation, not a fingerprint check.
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));
        known.observe(peer(2), key(2), "Daniel", Monotonic(200));

        assert!(known.accept_change(peer(2)));

        assert_eq!(known.get(peer(2)).unwrap().trust, TrustState::Unverified);
    }

    #[test]
    fn peers_are_listed_most_recently_seen_first() {
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));
        known.observe(peer(2), key(2), "Sarah", Monotonic(500));
        known.observe(peer(3), key(3), "Michael", Monotonic(300));

        let order: Vec<&str> =
            known.all().iter().map(|p| p.display_name.as_str()).collect();
        assert_eq!(order, vec!["Sarah", "Michael", "Daniel"]);
    }

    #[test]
    fn forgetting_a_peer_removes_them_entirely() {
        let mut known = KnownPeers::new();
        known.observe(peer(1), key(1), "Daniel", Monotonic(100));

        assert!(known.forget(peer(1)).is_some());
        assert!(!known.is_known(peer(1)));
        assert!(known.is_empty());

        // ...and meeting them again is a genuine first contact.
        assert_eq!(
            known.observe(peer(1), key(1), "Daniel", Monotonic(200)),
            TofuOutcome::FirstContact
        );
    }
}
