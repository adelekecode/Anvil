// Application state, folded from the core's event stream.
//
// This is the only place in the Flutter app that interprets events. Widgets read
// from here and call methods on it; none of them touch [AnvilApi] directly, so
// there is one place to look when the UI and the protocol disagree.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../models/anvil_event.dart';
import '../services/anvil_api.dart';

/// A pending trust warning the user has to answer.
class IdentityWarning {
  const IdentityWarning({
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

/// Holds everything the UI renders.
class AnvilController extends ChangeNotifier {
  AnvilController(this._api) {
    _subscription = _api.events.listen(_apply);
  }

  final AnvilApi _api;
  late final StreamSubscription<AnvilEvent> _subscription;

  Profile? _profile;
  AnvilState _state = AnvilState.initializing;
  RoomSnapshot? _room;

  final Map<String, DiscoveredPeer> _peers = {};
  final Map<String, String> _transports = {};
  final Map<String, List<ChatMessage>> _conversations = {};
  final Map<String, int> _unread = {};

  CallPhase _callPhase = CallPhase.idle;
  String? _callPeerId;
  String? _callPeerName;
  String? _callEndedMessage;

  IdentityWarning? _identityWarning;
  bool _muted = false;
  String? _lastError;
  Map<String, dynamic>? _diagnostics;

  // --- identity ---------------------------------------------------------

  /// This device's identity, or null before first run has completed.
  ///
  /// Its absence is the only thing gating the app. There is no login.
  Profile? get profile => _profile;

  /// Whether first run is still outstanding.
  bool get needsProfile => _profile == null;

  // --- state ------------------------------------------------------------

  AnvilState get state => _state;
  RoomSnapshot? get room => _room;
  bool get muted => _muted;
  String? get lastError => _lastError;
  Map<String, dynamic>? get diagnostics => _diagnostics;
  IdentityWarning? get identityWarning => _identityWarning;

  /// Whether media is flowing normally.
  bool get isSettled => _state == AnvilState.connected;

  // --- peers ------------------------------------------------------------

  /// Nearby peers: known contacts first, then hosts, then by name.
  ///
  /// That order is deliberate — the person you have spoken to before is almost
  /// always who you are looking for.
  List<DiscoveredPeer> get peers {
    final list = _peers.values.toList()
      ..sort((a, b) {
        if (a.known != b.known) return a.known ? -1 : 1;
        if (a.hostingRoom != b.hostingRoom) return a.hostingRoom ? -1 : 1;
        return a.displayName.toLowerCase().compareTo(b.displayName.toLowerCase());
      });
    return list;
  }

  DiscoveredPeer? peerByFingerprint(String fingerprint) => _peers[fingerprint];

  DiscoveredPeer? peerById(String peerId) {
    for (final peer in _peers.values) {
      if (peer.peerId == peerId) return peer;
    }
    return null;
  }

  /// Which transport is carrying media for a peer, for the diagnostics view.
  String? transportFor(String peerId) => _transports[peerId];

  // --- calls ------------------------------------------------------------

  CallPhase get callPhase => _callPhase;
  String? get callPeerId => _callPeerId;

  /// Best available name for the other party.
  String get callPeerName =>
      _callPeerName ?? (_callPeerId == null ? '' : peerById(_callPeerId!)?.displayName ?? 'Unknown');

  /// Set when a call ends, so the UI can say why. Cleared on acknowledgement.
  String? get callEndedMessage => _callEndedMessage;

  // --- chat -------------------------------------------------------------

  /// Messages in a conversation, oldest first.
  List<ChatMessage> messages(ConversationRef conversation) =>
      _conversations[conversation.key] ?? const [];

  /// Unread count for a conversation.
  int unread(ConversationRef conversation) => _unread[conversation.key] ?? 0;

  /// Mark a conversation read.
  void markRead(ConversationRef conversation) {
    if (_unread.remove(conversation.key) != null) notifyListeners();
  }

  // --- actions ----------------------------------------------------------

  void createProfile(String displayName) => _api.createProfile(displayName);
  void renameProfile(String displayName) => _api.renameProfile(displayName);

  void startDiscovery() => _api.startDiscovery();
  void stopDiscovery() => _api.stopDiscovery();

  void call(String peerId) => _api.callPeer(peerId);
  void acceptCall() => _api.acceptCall();
  void declineCall() => _api.declineCall();
  void endCall() => _api.endCall();

  void sendMessage(ConversationRef conversation, String body) {
    if (body.trim().isEmpty) return;
    if (conversation.isDirect) {
      _api.sendDirectMessage(conversation.peerId!, body);
    } else {
      _api.sendRoomMessage(conversation.roomId!, body);
    }
  }

  void createRoom() => _api.createRoom();
  void joinRoomByCode(String code) => _api.joinRoomByCode(code);
  void leaveRoom() => _api.leaveRoom();
  void requestDiagnostics() => _api.requestDiagnostics();

  void toggleMute() => _muted ? _api.unmute() : _api.mute();

  void verifyPeer(String peerId) {
    _api.verifyPeer(peerId);
    _clearWarningFor(peerId);
  }

  void acceptIdentityChange(String peerId) {
    _api.acceptIdentityChange(peerId);
    _clearWarningFor(peerId);
  }

  void clearError() {
    _lastError = null;
    notifyListeners();
  }

  void clearCallEndedMessage() {
    _callEndedMessage = null;
    notifyListeners();
  }

  // --- event folding ----------------------------------------------------

  void _apply(AnvilEvent event) {
    switch (event) {
      case ProfileReady(:final profile):
        _profile = profile;

      case StateChanged(:final state):
        _state = state;

      case PeerDiscovered(:final peer):
        // Preserve anything we already learned about them — a fresh
        // advertisement carries less than the confirmed record does.
        final existing = _peers[peer.fingerprint];
        _peers[peer.fingerprint] = existing == null
            ? peer
            : peer.copyWith(
                peerId: existing.peerId,
                trust: existing.trust,
                known: existing.known,
                confirmed: existing.confirmed || peer.confirmed,
              );

      case PeerLost(:final peerId):
        _peers.removeWhere((_, peer) => peer.fingerprint == peerId);

      case IdentityChanged(
          :final peerId,
          :final displayName,
          :final previousFingerprint,
          :final newFingerprint
        ):
        // Not an error banner. This needs a decision from the user, so it gets
        // its own surface and stays until they answer it.
        _identityWarning = IdentityWarning(
          peerId: peerId,
          displayName: displayName,
          previousFingerprint: previousFingerprint,
          newFingerprint: newFingerprint,
        );
        _updatePeerById(peerId, (peer) => peer.copyWith(trust: PeerTrust.changed));

      case PeerRenamed(:final peerId, :final displayName):
        _updatePeerById(peerId, (peer) => peer.copyWith(displayName: displayName));

      case IncomingCall(:final peerId, :final displayName):
        _callPhase = CallPhase.incoming;
        _callPeerId = peerId;
        _callPeerName = displayName;

      case CallStateChanged(:final phase, :final peerId, :final displayName):
        _callPhase = phase;
        _callPeerId = peerId ?? (phase == CallPhase.idle ? null : _callPeerId);
        if (displayName != null) _callPeerName = displayName;
        if (phase == CallPhase.idle) _callPeerName = null;

      case CallFinished(:final reason):
        _callPhase = CallPhase.idle;
        _callEndedMessage = reason.description;
        _callPeerId = null;
        _callPeerName = null;

      case MessageReceived(:final message):
        if (message != null) _recordMessage(message);

      case MessageDeliveryChanged(
          :final id,
          :final delivery,
          :final deliveredCount,
          :final totalCount
        ):
        _updateDelivery(id, delivery, deliveredCount, totalCount);

      case RoomCreated(:final roomId, :final shortId, :final joinCode):
        // The join code arrives with creation and nowhere else, so hold it
        // until the room snapshot catches up.
        _room = RoomSnapshot(
          roomId: roomId,
          shortId: shortId,
          epoch: 0,
          isHost: true,
          isDirect: true,
          relay: null,
          participants: const [],
          joinCode: joinCode,
        );

      case RoomJoined(:final room):
        _room = room.joinCode == null && _room?.joinCode != null
            ? room.copyWith(joinCode: _room!.joinCode)
            : room;

      case RoomLeft():
        _room = null;
        _transports.clear();

      case ParticipantJoined(:final participant):
        _updateRoom((room) => [...room.participants, participant]);

      case ParticipantLeft(:final peerId):
        _updateRoom(
            (room) => room.participants.where((p) => p.peerId != peerId).toList());

      case SpeakingChanged(:final peerId, :final speaking):
        _updateRoom((room) => room.participants
            .map((p) => p.peerId == peerId ? p.copyWith(speaking: speaking) : p)
            .toList());

      case TransportChanged(:final peerId, :final active):
        _transports[peerId] = active;

      case RelayChanged(:final relay):
        final current = _room;
        if (current != null) _room = current.copyWith(relay: relay);

      case MuteChanged(:final muted):
        _muted = muted;

      case ErrorEvent(:final layer, :final message):
        _lastError = '$layer: $message';

      case DiagnosticsEvent(:final data):
        _diagnostics = data;

      case PermissionRequired(:final capability):
        _lastError = 'permission required: $capability';

      case EpochChanged():
      case UnknownEvent():
        break;
    }

    notifyListeners();
  }

  void _recordMessage(ChatMessage message) {
    final key = message.conversation.key;
    final existing = _conversations[key] ?? const <ChatMessage>[];

    // Idempotent: a room message can reach us directly and via the relay.
    if (existing.any((m) => m.id == message.id)) return;

    _conversations[key] = [...existing, message];
    if (!message.outbound) {
      _unread[key] = (_unread[key] ?? 0) + 1;
    }
  }

  void _updateDelivery(
    String id,
    MessageDelivery delivery,
    int deliveredCount,
    int totalCount,
  ) {
    for (final entry in _conversations.entries) {
      final index = entry.value.indexWhere((m) => m.id == id);
      if (index == -1) continue;

      final updated = [...entry.value];
      updated[index] = updated[index].copyWith(
        delivery: delivery,
        deliveredCount: deliveredCount,
        totalCount: totalCount,
      );
      _conversations[entry.key] = updated;
      return;
    }
  }

  void _updatePeerById(
    String peerId,
    DiscoveredPeer Function(DiscoveredPeer) transform,
  ) {
    for (final entry in _peers.entries) {
      if (entry.value.peerId == peerId) {
        _peers[entry.key] = transform(entry.value);
        return;
      }
    }
  }

  void _updateRoom(List<Participant> Function(RoomSnapshot) transform) {
    final current = _room;
    if (current == null) return;
    _room = current.copyWith(participants: transform(current));
  }

  void _clearWarningFor(String peerId) {
    if (_identityWarning?.peerId == peerId) _identityWarning = null;
    _updatePeerById(peerId, (peer) => peer.copyWith(trust: PeerTrust.unverified));
    notifyListeners();
  }

  @override
  void dispose() {
    unawaited(_subscription.cancel());
    super.dispose();
  }
}
