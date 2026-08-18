//! Cross-module scenario tests.
//!
//! Unit tests in each module check one thing in isolation. These check that the
//! pieces agree with each other, using only the public API — which is also a
//! check that the public API is enough to build the product with.
//!
//! Each test corresponds to a scenario from the architecture spec, and every one
//! of them runs with no device, no network and no real time. That is the payoff
//! of injecting the clock and the platform: the failover and election paths,
//! which are miserable to reproduce on real hardware, are ordinary tests here.

use core::time::Duration;

#[cfg(feature = "crypto")]
use anvil_core::crypto::GroupKeyManager;
use anvil_core::discovery::{Advertisement, PeerAdvertisement, PeerTable};
use anvil_core::protocol::{MediaHeader, MediaPacket, PacketType};
use anvil_core::relay::{decide, elect, ElectionReason, ForwardDecision, RelayCandidate};
use anvil_core::room::{AdmissionPolicy, Participant, RoomState};
use anvil_core::routing::{resolve_media, Topology};
use anvil_core::transport::{Endpoint, PathKind, PathSample, SwitchDecision, TransportManager};
use anvil_core::{Epoch, MediaTimestamp, Monotonic, PeerId, RelayConfig, SeqNum, TransportConfig};

fn peer(n: u8) -> PeerId {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    PeerId(bytes)
}

fn participant(n: u8) -> Participant {
    Participant::new(peer(n), format!("peer{n}"), Monotonic::ZERO)
}

/// §97 — the adaptive transport test.
///
/// Same LAN plus Wi-Fi Aware available; LAN wins. Router disappears; Aware takes
/// over. The room, the participants and the key epoch are untouched throughout —
/// which is the entire point of §21.
#[test]
fn losing_the_router_mid_call_does_not_disturb_the_room() {
    let now = Monotonic(1_000);
    let mut room = RoomState::create(peer(1), "Alice".into(), AdmissionPolicy::Open, now);
    room.add_participant(participant(2), Epoch(1)).unwrap();

    let room_id_before = room.room_id;
    let epoch_before = room.epoch;
    let members_before: Vec<PeerId> = room.participants.keys().copied().collect();

    // Both paths to Bob come up.
    let mut transport = TransportManager::new(TransportConfig::default());
    let lan =
        transport.add_candidate(peer(2), Endpoint::new(PathKind::Lan, "10.0.0.2:47820"), 0, now);
    let aware =
        transport.add_candidate(peer(2), Endpoint::new(PathKind::WifiAware, "aware:2"), 0, now);

    for (path, rtt) in [(lan, 4), (aware, 8)] {
        transport.on_established(path, 1_200, now);
        transport.on_sample(path, PathSample::Rtt(Duration::from_millis(rtt)), now);
        transport.on_sample(path, PathSample::Delivery { expected: 100, received: 100 }, now);
    }

    transport.evaluate(now);
    assert_eq!(
        transport.active_path(peer(2)).map(|p| p.kind),
        Some(PathKind::Lan),
        "LAN should win when both paths are healthy"
    );

    // Router unplugged, one second into the call — well inside min_dwell.
    let outage = Monotonic(2_000);
    assert_eq!(transport.on_lost(lan, outage), Some(peer(2)));

    let changes = transport.evaluate(outage);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].decision, SwitchDecision::Failover);
    assert_eq!(
        transport.active_path(peer(2)).map(|p| p.kind),
        Some(PathKind::WifiAware),
        "media should have moved to Wi-Fi Aware"
    );

    // Nothing above the transport layer changed.
    assert_eq!(room.room_id, room_id_before);
    assert_eq!(room.epoch, epoch_before);
    assert_eq!(room.participants.keys().copied().collect::<Vec<_>>(), members_before);
}

/// §98 — the relay failure test.
///
/// Four participants, Chris relaying. Chris disappears. A new relay is elected
/// and, crucially, **the key epoch does not advance** — the relay held no key
/// material, so replacing it costs nothing cryptographically.
#[test]
fn losing_the_relay_elects_a_new_one_without_rekeying() {
    let config = RelayConfig::default();
    let mut room =
        RoomState::create(peer(1), "Alice".into(), AdmissionPolicy::Open, Monotonic::ZERO);
    room.add_participant(participant(2), Epoch(1)).unwrap();
    room.add_participant(participant(3), Epoch(2)).unwrap();
    room.add_participant(participant(4), Epoch(3)).unwrap();
    room.set_relay(Some(peer(3))).unwrap();

    let epoch_before = room.epoch;

    let candidates = [
        RelayCandidate {
            peer: peer(1),
            reachable_members: 4,
            network_quality: 85.0,
            stability: 90.0,
            capability: 80.0,
            battery_pct: Some(70),
            charging: false,
        },
        RelayCandidate {
            peer: peer(2),
            reachable_members: 4,
            network_quality: 70.0,
            stability: 80.0,
            capability: 70.0,
            battery_pct: Some(60),
            charging: false,
        },
    ];

    let result = elect(
        &candidates,
        Some(peer(3)),
        Monotonic::ZERO,
        true, // Chris has failed
        4,
        &config,
        Monotonic(5_000),
    )
    .expect("a failed relay must be replaced");

    assert_eq!(result.reason, ElectionReason::RelayFailed);
    room.set_relay(Some(result.relay)).unwrap();

    assert_eq!(room.epoch, epoch_before, "a relay change must not force a rekey");
    assert_eq!(room.size(), 4, "the room must survive its relay leaving");
}

/// §99 — the security test, in the parts that are testable without live crypto.
///
/// A relay sees routing information and forwards sealed bytes. It cannot forward
/// anything that would give it a say in the room, and it cannot alter a packet's
/// sequence or epoch without breaking authentication.
#[test]
fn a_relay_forwards_media_and_nothing_that_confers_authority() {
    let members = [peer(1), peer(2), peer(3), peer(4)];

    let media = MediaHeader::new(
        PacketType::Media,
        7,
        peer(1).route_id(),
        0,
        SeqNum(100),
        MediaTimestamp(96_000),
        Epoch(3),
    );

    match decide(&media, &members, Some(peer(1))) {
        ForwardDecision::Forward { to } => assert_eq!(to, vec![peer(2), peer(3), peer(4)]),
        other => panic!("media should fan out, got {other:?}"),
    }

    for kind in [
        PacketType::KeyExchange,
        PacketType::KeyRotate,
        PacketType::Membership,
        PacketType::RelaySwitch,
        PacketType::RoomAccept,
        PacketType::Identity,
    ] {
        let header = MediaHeader::new(
            kind,
            7,
            peer(1).route_id(),
            0,
            SeqNum(1),
            MediaTimestamp(0),
            Epoch(3),
        );
        assert!(
            matches!(decide(&header, &members, Some(peer(1))), ForwardDecision::Drop { .. }),
            "{kind:?} must never be relayable"
        );
    }

    // Tampering with a routing field the relay can see changes the AEAD
    // associated data, so the receiver's authentication would fail.
    let mut tampered = media;
    tampered.sequence = SeqNum(50);
    assert_ne!(media.associated_data(), tampered.associated_data());

    // Marking the packet relayed does not, so a relay can do its job without a
    // key.
    let mut relayed = media;
    assert!(relayed.mark_relayed());
    assert_eq!(media.associated_data(), relayed.associated_data());
    assert!(
        matches!(decide(&relayed, &members, Some(peer(1))), ForwardDecision::Drop { .. }),
        "a packet must never be forwarded twice"
    );
}

/// §65 — one phone found over two transports is one person, with two paths.
///
/// This is what makes failover possible at all: treating the same device as two
/// peers would give each of them one path and no standby.
#[test]
fn a_peer_found_twice_becomes_one_peer_with_two_paths() {
    let fingerprint = [0xAB; 8];
    let mut table = PeerTable::new();
    let mut transport = TransportManager::new(TransportConfig::default());

    for (kind, address) in [(PathKind::Lan, "10.0.0.7:47820"), (PathKind::WifiAware, "aware:7")] {
        table.observe(
            &PeerAdvertisement {
                kind,
                handle: address.into(),
                endpoint: Endpoint::new(kind, address),
                advertisement: Advertisement::new(fingerprint, None, "Chris"),
            },
            Monotonic(500),
        );
    }

    assert_eq!(table.len(), 1);
    let discovered = table.get(&fingerprint).unwrap().clone();
    assert_eq!(discovered.kinds(), vec![PathKind::Lan, PathKind::WifiAware]);

    // Both endpoints become candidate paths for the same peer.
    table.confirm(fingerprint, peer(7), "Chris".into());
    for (kind, endpoint) in discovered.endpoints {
        let path = transport.add_candidate(peer(7), endpoint, 0, Monotonic(500));
        transport.on_established(path, 1_000, Monotonic(600));
        transport.on_sample(path, PathSample::Rtt(Duration::from_millis(5)), Monotonic(600));
        let _ = kind;
    }
    transport.evaluate(Monotonic(600));

    let connection = transport.connection(peer(7)).expect("peer should be reachable");
    assert!(connection.active.is_some(), "no active path");
    assert!(connection.standby.is_some(), "no standby path — failover would be impossible");
}

/// §35, §74 — a sender uploads one stream regardless of room size.
#[test]
fn relayed_rooms_keep_the_senders_upload_at_one_stream() {
    let mut transport = TransportManager::new(TransportConfig::default());
    for n in [2u8, 3, 4] {
        let path = transport.add_candidate(
            peer(n),
            Endpoint::new(PathKind::Lan, "10.0.0.1:1"),
            0,
            Monotonic::ZERO,
        );
        transport.on_established(path, 1_200, Monotonic(100));
        transport.on_sample(path, PathSample::Rtt(Duration::from_millis(4)), Monotonic(100));
    }
    transport.evaluate(Monotonic(100));

    let members = [peer(1), peer(2), peer(3), peer(4)];
    let topology = Topology::resolve(peer(1), &members, Some(peer(2)));
    let routes = resolve_media(&topology, &members, peer(1), &transport);

    assert_eq!(routes.len(), 1, "a sender must upload once, not once per listener");
    assert_eq!(routes[0].peer, peer(2));

    // Two people need no relay at all.
    let pair = [peer(1), peer(2)];
    assert_eq!(
        Topology::resolve(peer(1), &pair, Some(peer(2))),
        Topology::Direct { peer: peer(2) }
    );
}

/// §50 — a departed member cannot decrypt anything sent after they left.
#[cfg(feature = "crypto")]
#[test]
fn departure_removes_key_material() {
    use anvil_core::crypto::SenderKeyManager;

    let mut keys = SenderKeyManager::new(peer(1));
    keys.install_member_key(peer(2), Epoch(0), &[7u8; 32]).unwrap();
    keys.install_member_key(peer(3), Epoch(0), &[8u8; 32]).unwrap();
    assert!(keys.can_decrypt(peer(2)));

    // Peer 3 leaves; the epoch advances with peer 3 absent from the member list.
    keys.rotate(Epoch(1), &[peer(1), peer(2)]).unwrap();
    keys.install_member_key(peer(2), Epoch(1), &[9u8; 32]).unwrap();
    keys.remove_member(peer(3));

    assert!(keys.can_decrypt(peer(2)));
    assert!(!keys.can_decrypt(peer(3)), "a departed member still had key material");
    assert_eq!(keys.epoch(), Epoch(1));
}

/// Hostile bytes on the wire never panic and never reach anything holding keys.
#[test]
fn malformed_packets_are_rejected_at_the_parser() {
    let valid = MediaPacket::new(
        MediaHeader::new(PacketType::Media, 1, 2, 0, SeqNum(1), MediaTimestamp(0), Epoch(0)),
        vec![0xAA; 80],
    )
    .encode();

    // Every truncation.
    for cut in 0..valid.len() {
        let _ = MediaPacket::decode(&valid[..cut]);
    }

    // Arbitrary bytes at a spread of lengths.
    for len in 0..128usize {
        for seed in 0..8u8 {
            let bytes: Vec<u8> =
                (0..len).map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed)).collect();
            let _ = MediaPacket::decode(&bytes);
        }
    }

    // A packet with no room for an authentication tag is refused before any
    // key-handling code sees it.
    let no_tag = MediaPacket::new(
        MediaHeader::new(PacketType::Media, 1, 2, 0, SeqNum(1), MediaTimestamp(0), Epoch(0)),
        vec![0xAA; 4],
    )
    .encode();
    assert!(MediaPacket::decode(&no_tag).is_err());
}
