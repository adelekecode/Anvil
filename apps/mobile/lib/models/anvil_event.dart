// Dart mirrors of the events the Rust core emits.
//
// The JSON schema is produced by `crates/anvil-ffi/src/convert.rs`. Both sides
// have tests over the same field names; if you rename one here, rename it there
// in the same change.

/// Node state, mirroring `AppState` in the core.
enum AnvilState {
  initializing,
  permissionsRequired,
  idle,
  discovering,
  creatingRoom,
  joiningRoom,
  connected,
  reconnecting,
  relayElection,
  leaving,
  error;

  static AnvilState parse(String? name) => AnvilState.values.firstWhere(
        (state) => state.name == name,
        orElse: () => AnvilState.error,
      );

  /// Whether the UI should show the room screen.
  bool get inRoom =>
      this == connected || this == reconnecting || this == relayElection;

  /// The room is alive but the network is being reorganised. Worth telling the
  /// user, worth not alarming them.
  bool get isUnsettled => this == reconnecting || this == relayElection;
}

/// This device's identity.
///
/// Generated locally on first run from nothing but a display name. There is no
/// account behind it, no server that knows about it, and no way to recover it
/// if the device is lost — which is the trade for having no account at all.
class Profile {
  const Profile({
    required this.displayName,
    required this.peerId,
    required this.shortPeerId,
    required this.fingerprint,
    required this.fingerprintLong,
    required this.protocolVersion,
  });

  /// What humans call this device. Not unique, not an identity.
  final String displayName;

  /// Full `anv_…` identifier. This is the identity.
  final String peerId;

  /// Abbreviated form, marked with an ellipsis so nobody compares two of them
  /// and believes they compared identities.
  final String shortPeerId;

  /// Short fingerprint for reading aloud: `7A:42:19:BC`.
  final String fingerprint;

  /// Grouped long fingerprint, for a verification screen.
  final String fingerprintLong;

  final int protocolVersion;

  factory Profile.fromJson(Map<String, dynamic> json) => Profile(
        displayName: json['displayName'] as String? ?? '',
        peerId: json['peerId'] as String? ?? '',
        shortPeerId: json['shortPeerId'] as String? ?? '',
        fingerprint: json['fingerprint'] as String? ?? '',
        fingerprintLong: json['fingerprintLong'] as String? ?? '',
        protocolVersion: json['protocolVersion'] as int? ?? 0,
      );
}

/// How much has been confirmed about a peer.
enum PeerTrust {
  /// Met before, never confirmed out of band. The normal state.
  unverified,

  /// Confirmed in person or by QR.
  verified,

  /// Presented a different key for a name we already trusted. The warning case.
  changed;

  bool get needsWarning => this == changed;
}

/// A peer visible nearby.
class DiscoveredPeer {
  const DiscoveredPeer({
    required this.fingerprint,
    required this.displayName,
    required this.confirmed,
    required this.hostingRoom,
    required this.transports,
    this.peerId,
    this.rttMs,
    this.trust,
    this.known = false,
  });

  /// Provisional correlation key. Not an identity.
  final String fingerprint;

  /// Advertised name.
  ///
  /// **Unverified while [confirmed] is false.** Anyone in radio range can
  /// advertise any name.
  final String displayName;

  /// Whether identity has been cryptographically confirmed.
  final bool confirmed;

  /// Whether this peer is advertising a joinable room.
  final bool hostingRoom;

  /// Transports this peer was found on.
  final List<String> transports;

  /// Full identity, once confirmed.
  final String? peerId;

  /// Round-trip time, once measured. Null means unmeasured — which the UI must
  /// render differently from a fast zero.
  final int? rttMs;

  /// Trust state, for peers we have met before.
  final PeerTrust? trust;

  /// Whether this device has met them before.
  final bool known;

  bool get needsWarning => trust?.needsWarning ?? false;

  factory DiscoveredPeer.fromJson(Map<String, dynamic> json) => DiscoveredPeer(
        fingerprint: json['fingerprint'] as String? ?? '',
        displayName: json['displayName'] as String? ?? 'Unknown',
        confirmed: json['confirmed'] as bool? ?? false,
        hostingRoom: json['hostingRoom'] as bool? ?? false,
        transports: (json['transports'] as List?)?.cast<String>() ?? const [],
        peerId: json['peerId'] as String?,
        rttMs: json['rttMs'] as int?,
        known: json['known'] as bool? ?? false,
      );

  DiscoveredPeer copyWith({
    String? peerId,
    int? rttMs,
    PeerTrust? trust,
    bool? known,
    bool? confirmed,
    String? displayName,
  }) =>
      DiscoveredPeer(
        fingerprint: fingerprint,
        displayName: displayName ?? this.displayName,
        confirmed: confirmed ?? this.confirmed,
        hostingRoom: hostingRoom,
        transports: transports,
        peerId: peerId ?? this.peerId,
        rttMs: rttMs ?? this.rttMs,
        trust: trust ?? this.trust,
        known: known ?? this.known,
      );
}

/// Someone in the room.
class Participant {
  const Participant({
    required this.peerId,
    required this.displayName,
    required this.speaking,
    required this.muted,
  });

  final String peerId;
  final String displayName;
  final bool speaking;
  final bool muted;

  factory Participant.fromJson(Map<String, dynamic> json) => Participant(
        peerId: json['peerId'] as String? ?? '',
        displayName: json['displayName'] as String? ?? 'Unknown',
        speaking: json['speaking'] as bool? ?? false,
        muted: json['muted'] as bool? ?? false,
      );

  Participant copyWith({bool? speaking, bool? muted}) => Participant(
        peerId: peerId,
        displayName: displayName,
        speaking: speaking ?? this.speaking,
        muted: muted ?? this.muted,
      );
}

/// A room as the UI sees it.
class RoomSnapshot {
  const RoomSnapshot({
    required this.roomId,
    required this.shortId,
    required this.epoch,
    required this.isHost,
    required this.isDirect,
    required this.relay,
    required this.participants,
    this.joinCode,
  });

  final String roomId;
  final String shortId;
  final int epoch;
  final bool isHost;

  /// Whether media is peer-to-peer rather than relayed.
  final bool isDirect;

  final String? relay;
  final List<Participant> participants;

  /// The code to read out. Only the host has it at creation; joiners typed it.
  final String? joinCode;

  factory RoomSnapshot.fromJson(Map<String, dynamic> json) => RoomSnapshot(
        roomId: json['roomId'] as String? ?? '',
        shortId: json['shortId'] as String? ?? '',
        epoch: json['epoch'] as int? ?? 0,
        isHost: json['isHost'] as bool? ?? false,
        isDirect: json['isDirect'] as bool? ?? false,
        relay: json['relay'] as String?,
        participants: ((json['participants'] as List?) ?? const [])
            .map((p) => Participant.fromJson(p as Map<String, dynamic>))
            .toList(),
        joinCode: json['joinCode'] as String?,
      );

  RoomSnapshot copyWith({
    String? relay,
    List<Participant>? participants,
    String? joinCode,
  }) =>
      RoomSnapshot(
        roomId: roomId,
        shortId: shortId,
        epoch: epoch,
        isHost: isHost,
        isDirect: isDirect,
        relay: relay ?? this.relay,
        participants: participants ?? this.participants,
        joinCode: joinCode ?? this.joinCode,
      );
}

/// Where a direct call has got to.
enum CallPhase {
  idle,
  outgoing,
  incoming,
  active;

  static CallPhase parse(String? name) => CallPhase.values.firstWhere(
        (phase) => phase.name == name,
        orElse: () => CallPhase.idle,
      );

  bool get isBusy => this != idle;
}

/// Why a call ended. Distinct reasons because the UI should say different
/// things — hanging up is a decision, losing a peer is a network event.
enum CallEndReason {
  cancelled,
  declined,
  hungUp,
  unanswered,
  unreachable;

  static CallEndReason parse(String? name) => CallEndReason.values.firstWhere(
        (reason) => reason.name == name,
        orElse: () => CallEndReason.hungUp,
      );

  String get description => switch (this) {
        cancelled => 'Call cancelled',
        declined => 'Call declined',
        hungUp => 'Call ended',
        unanswered => 'No answer',
        unreachable => 'Lost connection',
      };
}

/// How far an outbound message got.
enum MessageDelivery {
  pending,
  sent,
  delivered,
  undeliverable,
  partial;

  static MessageDelivery parse(String? name) =>
      MessageDelivery.values.firstWhere(
        (delivery) => delivery.name == name,
        orElse: () => MessageDelivery.pending,
      );

  bool get isFailure => this == undeliverable;
  bool get inFlight => this == pending || this == sent;
}

/// A conversation: one peer, or one room.
class ConversationRef {
  const ConversationRef.direct(this.peerId) : roomId = null;
  const ConversationRef.room(this.roomId) : peerId = null;

  final String? peerId;
  final String? roomId;

  bool get isDirect => peerId != null;

  /// Stable map key.
  String get key => peerId != null ? 'peer:$peerId' : 'room:$roomId';

  static ConversationRef? fromJson(Map<String, dynamic>? json) {
    if (json == null) return null;
    final peerId = json['peerId'] as String?;
    if (peerId != null) return ConversationRef.direct(peerId);
    final roomId = json['roomId'] as String?;
    if (roomId != null) return ConversationRef.room(roomId);
    return null;
  }

  @override
  bool operator ==(Object other) =>
      other is ConversationRef && other.key == key;

  @override
  int get hashCode => key.hashCode;
}

/// One message.
class ChatMessage {
  const ChatMessage({
    required this.id,
    required this.from,
    required this.conversation,
    required this.body,
    required this.atMs,
    required this.outbound,
    required this.delivery,
    required this.deliveredCount,
    required this.totalCount,
  });

  final String id;
  final String from;
  final ConversationRef conversation;
  final String body;

  /// Local monotonic milliseconds, not wall clock.
  ///
  /// With no internet there is no agreed time, and a sender-controlled
  /// timestamp would let anyone place a message anywhere in your history.
  final int atMs;

  final bool outbound;
  final MessageDelivery delivery;

  /// For room messages delivered to some members but not all.
  final int deliveredCount;
  final int totalCount;

  ChatMessage copyWith({
    MessageDelivery? delivery,
    int? deliveredCount,
    int? totalCount,
  }) =>
      ChatMessage(
        id: id,
        from: from,
        conversation: conversation,
        body: body,
        atMs: atMs,
        outbound: outbound,
        delivery: delivery ?? this.delivery,
        deliveredCount: deliveredCount ?? this.deliveredCount,
        totalCount: totalCount ?? this.totalCount,
      );

  static ChatMessage? fromJson(Map<String, dynamic> json) {
    final conversation =
        ConversationRef.fromJson(json['conversation'] as Map<String, dynamic>?);
    if (conversation == null) return null;

    final delivery = json['delivery'] as Map<String, dynamic>?;

    return ChatMessage(
      id: json['id'] as String? ?? '',
      from: json['from'] as String? ?? '',
      conversation: conversation,
      body: json['body'] as String? ?? '',
      atMs: json['atMs'] as int? ?? 0,
      outbound: json['outbound'] as bool? ?? false,
      delivery: MessageDelivery.parse(delivery?['state'] as String?),
      deliveredCount: delivery?['delivered'] as int? ?? 0,
      totalCount: delivery?['total'] as int? ?? 0,
    );
  }
}

/// Base class for everything the core emits.
sealed class AnvilEvent {
  const AnvilEvent();

  /// Decode one event.
  ///
  /// Unknown types become [UnknownEvent] rather than throwing: the core may be
  /// newer than the app, and an unrecognised event is not a reason to tear down
  /// a live call.
  factory AnvilEvent.fromJson(Map<String, dynamic> json) {
    return switch (json['type'] as String?) {
      'stateChanged' => StateChanged(AnvilState.parse(json['state'] as String?)),
      'profileReady' => ProfileReady(Profile.fromJson(json)),
      'identityChanged' => IdentityChanged(
          peerId: json['peerId'] as String? ?? '',
          displayName: json['displayName'] as String? ?? '',
          previousFingerprint: json['previousFingerprint'] as String? ?? '',
          newFingerprint: json['newFingerprint'] as String? ?? '',
        ),
      'peerRenamed' => PeerRenamed(
          peerId: json['peerId'] as String? ?? '',
          previousName: json['previousName'] as String? ?? '',
          displayName: json['displayName'] as String? ?? '',
        ),
      'peerDiscovered' => PeerDiscovered(DiscoveredPeer.fromJson(json)),
      'peerLost' => PeerLost(json['peerId'] as String? ?? ''),
      'incomingCall' => IncomingCall(
          peerId: json['peerId'] as String? ?? '',
          displayName: json['displayName'] as String? ?? '',
        ),
      'callStateChanged' => CallStateChanged(
          phase: CallPhase.parse(json['call'] as String?),
          peerId: json['peerId'] as String?,
          displayName: json['displayName'] as String?,
        ),
      'callFinished' => CallFinished(
          peerId: json['peerId'] as String?,
          reason: CallEndReason.parse(json['reason'] as String?),
        ),
      'messageReceived' => MessageReceived(ChatMessage.fromJson(json)),
      'messageDelivery' => MessageDeliveryChanged(
          id: json['id'] as String? ?? '',
          delivery: MessageDelivery.parse(
            (json['delivery'] as Map<String, dynamic>?)?['state'] as String?,
          ),
          deliveredCount:
              (json['delivery'] as Map<String, dynamic>?)?['delivered'] as int? ??
                  0,
          totalCount:
              (json['delivery'] as Map<String, dynamic>?)?['total'] as int? ?? 0,
        ),
      'roomCreated' => RoomCreated(
          roomId: json['roomId'] as String? ?? '',
          shortId: json['shortId'] as String? ?? '',
          joinCode: json['joinCode'] as String? ?? '',
        ),
      'roomJoined' => RoomJoined(RoomSnapshot.fromJson(json)),
      'roomLeft' => RoomLeft(json['reason'] as String? ?? ''),
      'participantJoined' => ParticipantJoined(Participant.fromJson(json)),
      'participantLeft' => ParticipantLeft(json['peerId'] as String? ?? ''),
      'speakingChanged' => SpeakingChanged(
          json['peerId'] as String? ?? '',
          json['speaking'] as bool? ?? false,
        ),
      'transportChanged' => TransportChanged(
          json['peerId'] as String? ?? '',
          json['active'] as String? ?? '',
          json['standby'] as String?,
        ),
      'relayChanged' => RelayChanged(
          json['relay'] as String?,
          json['reason'] as String? ?? '',
        ),
      'muteChanged' => MuteChanged(json['muted'] as bool? ?? false),
      'epochChanged' => EpochChanged(json['epoch'] as int? ?? 0),
      'permissionRequired' =>
        PermissionRequired(json['capability'] as String? ?? ''),
      'diagnostics' => DiagnosticsEvent(json),
      'error' => ErrorEvent(
          json['layer'] as String? ?? 'unknown',
          json['message'] as String? ?? '',
        ),
      _ => UnknownEvent(json['type'] as String? ?? 'unknown'),
    };
  }
}

class StateChanged extends AnvilEvent {
  const StateChanged(this.state);
  final AnvilState state;
}

/// The local identity is loaded. The only thing that leaves the first-run
/// screen — there is no login behind it.
class ProfileReady extends AnvilEvent {
  const ProfileReady(this.profile);
  final Profile profile;
}

/// A peer presented a different key for a name we already trusted.
class IdentityChanged extends AnvilEvent {
  const IdentityChanged({
    required this.peerId,
    required this.displayName,
    required this.previousFingerprint,
    required this.newFingerprint,
  });

  final String peerId;
  final String displayName;
  final String previousFingerprint;
  final String newFingerprint;
}

class PeerRenamed extends AnvilEvent {
  const PeerRenamed({
    required this.peerId,
    required this.previousName,
    required this.displayName,
  });

  final String peerId;
  final String previousName;
  final String displayName;
}

class PeerDiscovered extends AnvilEvent {
  const PeerDiscovered(this.peer);
  final DiscoveredPeer peer;
}

class PeerLost extends AnvilEvent {
  const PeerLost(this.peerId);
  final String peerId;
}

class IncomingCall extends AnvilEvent {
  const IncomingCall({required this.peerId, required this.displayName});
  final String peerId;
  final String displayName;
}

class CallStateChanged extends AnvilEvent {
  const CallStateChanged({
    required this.phase,
    this.peerId,
    this.displayName,
  });

  final CallPhase phase;
  final String? peerId;
  final String? displayName;
}

class CallFinished extends AnvilEvent {
  const CallFinished({this.peerId, required this.reason});
  final String? peerId;
  final CallEndReason reason;
}

class MessageReceived extends AnvilEvent {
  const MessageReceived(this.message);
  final ChatMessage? message;
}

class MessageDeliveryChanged extends AnvilEvent {
  const MessageDeliveryChanged({
    required this.id,
    required this.delivery,
    required this.deliveredCount,
    required this.totalCount,
  });

  final String id;
  final MessageDelivery delivery;
  final int deliveredCount;
  final int totalCount;
}

class RoomCreated extends AnvilEvent {
  const RoomCreated({
    required this.roomId,
    required this.shortId,
    required this.joinCode,
  });

  final String roomId;
  final String shortId;
  final String joinCode;
}

class RoomJoined extends AnvilEvent {
  const RoomJoined(this.room);
  final RoomSnapshot room;
}

class RoomLeft extends AnvilEvent {
  const RoomLeft(this.reason);
  final String reason;
}

class ParticipantJoined extends AnvilEvent {
  const ParticipantJoined(this.participant);
  final Participant participant;
}

class ParticipantLeft extends AnvilEvent {
  const ParticipantLeft(this.peerId);
  final String peerId;
}

class SpeakingChanged extends AnvilEvent {
  const SpeakingChanged(this.peerId, this.speaking);
  final String peerId;
  final bool speaking;
}

/// The transport carrying media changed.
///
/// Note what this event does *not* say: nothing about the room, the
/// participants, or the session restarting. The call continued through it.
class TransportChanged extends AnvilEvent {
  const TransportChanged(this.peerId, this.active, this.standby);
  final String peerId;
  final String active;
  final String? standby;
}

class RelayChanged extends AnvilEvent {
  const RelayChanged(this.relay, this.reason);
  final String? relay;
  final String reason;
}

class MuteChanged extends AnvilEvent {
  const MuteChanged(this.muted);
  final bool muted;
}

class EpochChanged extends AnvilEvent {
  const EpochChanged(this.epoch);
  final int epoch;
}

class PermissionRequired extends AnvilEvent {
  const PermissionRequired(this.capability);
  final String capability;
}

class DiagnosticsEvent extends AnvilEvent {
  const DiagnosticsEvent(this.data);
  final Map<String, dynamic> data;
}

/// Something failed, with the layer named so the UI can say something useful
/// rather than "connection error".
class ErrorEvent extends AnvilEvent {
  const ErrorEvent(this.layer, this.message);
  final String layer;
  final String message;
}

class UnknownEvent extends AnvilEvent {
  const UnknownEvent(this.type);
  final String type;
}
