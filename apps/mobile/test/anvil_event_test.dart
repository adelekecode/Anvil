// Tests over the JSON boundary with the Rust core.
//
// This is the one place the two languages have to agree by hand: field names
// here are produced by `crates/anvil-ffi/src/convert.rs`, and nothing checks
// them at compile time. The Rust side has matching tests; these are the other
// half of that pair.
//
// The other thing under test is tolerance. Events arrive from a core that may
// be a different version than the app, so decoding must degrade rather than
// throw — an unrecognised event is not a reason to tear down a live call.

import 'package:anvil/models/anvil_event.dart';
import 'package:anvil/util/initials.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('profile', () {
    test('decodes the identity created on first run', () {
      final event = AnvilEvent.fromJson({
        'type': 'profileReady',
        'displayName': 'Femi',
        'peerId': 'anv_${'7a' * 32}',
        'shortPeerId': 'anv_7a7a7…',
        'fingerprint': '7A:42:19:BC',
        'fingerprintLong': '7A42 19BC 3F08 1122',
        'protocolVersion': 1,
      });

      expect(event, isA<ProfileReady>());
      final profile = (event as ProfileReady).profile;
      expect(profile.displayName, 'Femi');
      expect(profile.fingerprint, '7A:42:19:BC');
      expect(profile.protocolVersion, 1);
    });
  });

  group('trust', () {
    test('an identity change carries both fingerprints', () {
      // Without both, the warning screen cannot show the user what changed —
      // only that something did, which is not actionable.
      final event = AnvilEvent.fromJson({
        'type': 'identityChanged',
        'peerId': 'anv_${'02' * 32}',
        'displayName': 'Daniel',
        'previousFingerprint': '92:A8:F3:71',
        'newFingerprint': '1C:04:BE:5A',
      });

      expect(event, isA<IdentityChanged>());
      final changed = event as IdentityChanged;
      expect(changed.previousFingerprint, '92:A8:F3:71');
      expect(changed.newFingerprint, '1C:04:BE:5A');
      expect(changed.displayName, 'Daniel');
    });

    test('trust states map to warnings correctly', () {
      expect(PeerTrust.changed.needsWarning, isTrue);
      expect(PeerTrust.verified.needsWarning, isFalse);
      expect(PeerTrust.unverified.needsWarning, isFalse);
    });
  });

  group('rooms', () {
    test('room creation carries the code a person reads out', () {
      final event = AnvilEvent.fromJson({
        'type': 'roomCreated',
        'roomId': 'ab' * 16,
        'shortId': 'ABCDEF',
        'joinCode': 'ANV-7FK2-P9W4',
      });

      expect(event, isA<RoomCreated>());
      expect((event as RoomCreated).joinCode, 'ANV-7FK2-P9W4');
    });

    test('a room snapshot survives missing optional fields', () {
      final room = RoomSnapshot.fromJson({
        'roomId': 'cd' * 16,
        'shortId': 'CDEFAB',
        'epoch': 3,
        'isHost': false,
        'isDirect': false,
        'participants': [
          {'peerId': 'anv_a', 'displayName': 'Sarah'},
        ],
      });

      expect(room.relay, isNull);
      expect(room.joinCode, isNull);
      expect(room.participants.single.displayName, 'Sarah');
      expect(room.participants.single.speaking, isFalse);
    });
  });

  group('calls', () {
    test('call phases decode, and unknown ones fall back to idle', () {
      expect(CallPhase.parse('outgoing'), CallPhase.outgoing);
      expect(CallPhase.parse('active'), CallPhase.active);
      expect(CallPhase.parse('something-new'), CallPhase.idle);
      expect(CallPhase.parse(null), CallPhase.idle);
    });

    test('end reasons have distinct user-facing descriptions', () {
      // These are different because the user should be told different things:
      // hanging up is a decision, losing a peer is a network event.
      final descriptions =
          CallEndReason.values.map((r) => r.description).toSet();
      expect(descriptions.length, CallEndReason.values.length);
      expect(CallEndReason.unreachable.description, isNot(
          CallEndReason.hungUp.description));
    });

    test('an incoming call carries who is calling', () {
      final event = AnvilEvent.fromJson({
        'type': 'incomingCall',
        'peerId': 'anv_${'03' * 32}',
        'displayName': 'Sarah',
      });

      expect(event, isA<IncomingCall>());
      expect((event as IncomingCall).displayName, 'Sarah');
    });
  });

  group('chat', () {
    test('a direct message decodes with its conversation and delivery', () {
      final event = AnvilEvent.fromJson({
        'type': 'messageReceived',
        'id': 'abcdef0123456789abcdef01',
        'from': 'anv_${'04' * 32}',
        'conversation': {'kind': 'direct', 'peerId': 'anv_${'04' * 32}'},
        'body': 'Are you around?',
        'atMs': 1234,
        'outbound': false,
        'delivery': {'state': 'delivered'},
      });

      expect(event, isA<MessageReceived>());
      final message = (event as MessageReceived).message!;
      expect(message.body, 'Are you around?');
      expect(message.conversation.isDirect, isTrue);
      expect(message.delivery, MessageDelivery.delivered);
      expect(message.outbound, isFalse);
    });

    test('an undeliverable message is marked as a failure', () {
      // With no server there is nothing to hold a message for an absent peer,
      // and the UI must say so rather than showing a hopeful clock forever.
      final event = AnvilEvent.fromJson({
        'type': 'messageDelivery',
        'id': 'abc',
        'delivery': {'state': 'undeliverable'},
      });

      final delivery = (event as MessageDeliveryChanged).delivery;
      expect(delivery, MessageDelivery.undeliverable);
      expect(delivery.isFailure, isTrue);
      expect(delivery.inFlight, isFalse);
    });

    test('partial room delivery carries the counts', () {
      final event = AnvilEvent.fromJson({
        'type': 'messageDelivery',
        'id': 'abc',
        'delivery': {'state': 'partial', 'delivered': 2, 'total': 3},
      }) as MessageDeliveryChanged;

      expect(event.delivery, MessageDelivery.partial);
      expect(event.deliveredCount, 2);
      expect(event.totalCount, 3);
      expect(event.delivery.isFailure, isFalse,
          reason: 'reaching most of a room is not a failure');
    });

    test('conversation references compare by value', () {
      expect(
        const ConversationRef.direct('anv_a'),
        const ConversationRef.direct('anv_a'),
      );
      expect(
        const ConversationRef.direct('anv_a'),
        isNot(const ConversationRef.room('anv_a')),
      );
    });
  });

  group('peers', () {
    test('a discovered peer is unconfirmed until proven', () {
      final event = AnvilEvent.fromJson({
        'type': 'peerDiscovered',
        'fingerprint': 'aabbccdd',
        'displayName': 'Daniel',
        'confirmed': false,
        'hostingRoom': false,
        'transports': ['lan', 'wifi-aware'],
      });

      final peer = (event as PeerDiscovered).peer;
      expect(peer.confirmed, isFalse);
      expect(peer.peerId, isNull);
      expect(peer.transports, ['lan', 'wifi-aware']);
      expect(peer.needsWarning, isFalse);
    });

    test('copyWith preserves what was already learned', () {
      // A fresh advertisement carries less than a confirmed record does, and
      // must not overwrite it.
      const peer = DiscoveredPeer(
        fingerprint: 'aabbccdd',
        displayName: 'Daniel',
        confirmed: true,
        hostingRoom: false,
        transports: ['lan'],
        peerId: 'anv_abc',
        trust: PeerTrust.verified,
        known: true,
      );

      final refreshed = peer.copyWith(rttMs: 4);

      expect(refreshed.peerId, 'anv_abc');
      expect(refreshed.trust, PeerTrust.verified);
      expect(refreshed.known, isTrue);
      expect(refreshed.rttMs, 4);
    });
  });

  group('tolerance', () {
    test('an unknown event type does not throw', () {
      final event = AnvilEvent.fromJson({'type': 'somethingFromTheFuture'});
      expect(event, isA<UnknownEvent>());
      expect((event as UnknownEvent).type, 'somethingFromTheFuture');
    });

    test('a missing type does not throw', () {
      expect(AnvilEvent.fromJson({}), isA<UnknownEvent>());
    });

    test('missing fields fall back rather than crashing a live call', () {
      final event = AnvilEvent.fromJson({'type': 'peerDiscovered'});
      final peer = (event as PeerDiscovered).peer;

      expect(peer.displayName, 'Unknown');
      expect(peer.transports, isEmpty);
    });

    test('a message with no conversation is dropped rather than half-built', () {
      final event = AnvilEvent.fromJson({
        'type': 'messageReceived',
        'body': 'orphan',
      });

      expect((event as MessageReceived).message, isNull);
    });

    test('app states decode, unknown ones become error', () {
      expect(AnvilState.parse('connected'), AnvilState.connected);
      expect(AnvilState.parse('relayElection'), AnvilState.relayElection);
      expect(AnvilState.parse('newState'), AnvilState.error);
    });

    test('reconnecting and relay election are shown as unsettled, not broken',
        () {
      // The room is alive through both. Telling the user it has failed would
      // be wrong, and telling them nothing would be worse.
      expect(AnvilState.reconnecting.isUnsettled, isTrue);
      expect(AnvilState.relayElection.isUnsettled, isTrue);
      expect(AnvilState.connected.isUnsettled, isFalse);

      expect(AnvilState.reconnecting.inRoom, isTrue);
      expect(AnvilState.idle.inRoom, isFalse);
    });
  });

  group('initials', () {
    test('handles names that come off the air from strangers', () {
      expect(initialOf('Femi'), 'F');
      expect(initialOf('  daniel '), 'D');
      expect(initialOf(''), '?');
      expect(initialOf('   '), '?');
      expect(initialOf('émile'), 'É');
      // A multi-byte first character must not be sliced in half.
      expect(initialOf('😀 hello'), isNotEmpty);
    });
  });
}
