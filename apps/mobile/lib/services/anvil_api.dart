// The Dart-facing Anvil client.
//
// Commands go in as method calls; events come out as a stream. That is the
// whole surface — nothing above this file knows the core is Rust, and nothing
// polls it for protocol state (§89).
//
// The event pump runs on a background isolate. `anvil_next_event` blocks on the
// Rust side, and blocking the Dart UI isolate would freeze the app between
// events. The isolate does nothing but pull JSON strings and forward them.

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';

import '../models/anvil_event.dart';
import 'anvil_bindings.dart';

/// Thrown when a command is rejected at the boundary.
class AnvilCommandException implements Exception {
  AnvilCommandException(this.code, this.command);

  final int code;
  final String command;

  @override
  String toString() =>
      'Anvil rejected "$command": ${AnvilResult.describe(code)}';
}

/// A running Anvil node.
class AnvilApi {
  AnvilApi._(this._bindings, this._session);

  final AnvilBindings _bindings;
  Pointer<AnvilSession> _session;

  final _events = StreamController<AnvilEvent>.broadcast();
  Isolate? _pump;
  ReceivePort? _pumpPort;
  bool _disposed = false;

  static const _platformChannel = MethodChannel('dev.anvil/platform');

  /// Events from the core.
  Stream<AnvilEvent> get events => _events.stream;

  /// Wire protocol version this build speaks.
  int get protocolVersion => _bindings.protocolVersion();

  /// Start a node.
  ///
  /// [displayName] is advertised to nearby devices and is not authenticated
  /// until a handshake completes — treat it as a hint, not a claim.
  static Future<AnvilApi> start({
    required String displayName,
    bool diagnostics = false,
  }) async {
    final bindings = AnvilBindings.load();

    final config = jsonEncode({
      'displayName': displayName,
      'diagnostics': diagnostics,
    });

    final session = withNativeString(config, bindings.init);
    if (session == nullptr) {
      throw StateError('Anvil failed to start');
    }

    final api = AnvilApi._(bindings, session);
    try {
      await _platformChannel.invokeMethod<void>('attach', {
        'session': session.address,
      });
      await api._startPump();
      return api;
    } catch (_) {
      bindings.shutdown(session);
      rethrow;
    }
  }

  // --- identity ---------------------------------------------------------

  /// Create the local identity on first run.
  ///
  /// The only thing the user is ever asked for. Everything else — keypair, peer
  /// id, fingerprint — is generated on the device. There is no account, nothing
  /// is sent anywhere, and this cannot fail for a reason outside the device.
  void createProfile(String displayName) =>
      _send({'type': 'createProfile', 'displayName': displayName});

  /// Change the display name. Does not touch the identity.
  void renameProfile(String displayName) =>
      _send({'type': 'renameProfile', 'displayName': displayName});

  /// Record that a peer's key was checked out of band.
  void verifyPeer(String peerId) =>
      _send({'type': 'verifyPeer', 'peerId': peerId});

  /// Accept a peer's changed identity without verifying it.
  void acceptIdentityChange(String peerId) =>
      _send({'type': 'acceptIdentityChange', 'peerId': peerId});

  // --- calls ------------------------------------------------------------

  /// Call a peer. Two people need no relay.
  void callPeer(String peerId) => _send({'type': 'callPeer', 'peerId': peerId});

  /// Answer the incoming call.
  void acceptCall() => _send({'type': 'acceptCall'});

  /// Refuse the incoming call.
  void declineCall() => _send({'type': 'declineCall'});

  /// Hang up.
  void endCall() => _send({'type': 'endCall'});

  // --- chat -------------------------------------------------------------

  /// Message a peer directly.
  void sendDirectMessage(String peerId, String body) =>
      _send({'type': 'sendMessage', 'peerId': peerId, 'body': body});

  /// Message everyone in a room.
  void sendRoomMessage(String roomId, String body) =>
      _send({'type': 'sendMessage', 'roomId': roomId, 'body': body});

  // --- discovery and rooms ----------------------------------------------

  /// Begin discovering nearby peers over every available transport.
  void startDiscovery() => _send({'type': 'startDiscovery'});

  /// Stop discovering.
  void stopDiscovery() => _send({'type': 'stopDiscovery'});

  /// Create and host a room.
  void createRoom() => _send({'type': 'createRoom'});

  /// Join a room by its full hex id, with an optional join code.
  void joinRoom(String roomId, {String? credential}) => _send({
        'type': 'joinRoom',
        'roomId': roomId,
        if (credential != null) 'credential': credential,
      });

  /// Join using the code the host read out.
  ///
  /// The raw string is passed through deliberately — normalisation (case,
  /// hyphens, `O` for `0`) happens in the core, so every host gets the same
  /// leniency instead of each reimplementing it slightly differently.
  void joinRoomByCode(String code) =>
      _send({'type': 'joinRoomByCode', 'code': code});

  /// Admit or refuse a pending join request when the room uses host approval.
  ///
  /// The host's UI calls this in response to [`JoinRequested`] being emitted.
  /// On `accept = true` the engine admits the peer and pushes a fresh sender
  /// key down (`AppControl::MediaKey`). On `accept = false` it drops the
  /// request and emits [`JoinDenied`] so the joiner's UI stops waiting.
  void respondToJoin(String peerId, {required bool accept}) => _send({
        'type': 'respondToJoin',
        'peerId': peerId,
        'accept': accept,
      });

  /// Leave the current room.
  void leaveRoom() => _send({'type': 'leaveRoom'});

  /// Mute the microphone.
  void mute() => _send({'type': 'mute'});

  /// Unmute.
  void unmute() => _send({'type': 'unmute'});

  /// Ask for a diagnostics snapshot now.
  void requestDiagnostics() => _send({'type': 'requestDiagnostics'});

  void _send(Map<String, dynamic> command) {
    if (_disposed) {
      throw StateError('Anvil session has been disposed');
    }

    final json = jsonEncode(command);
    final code = withNativeString(
      json,
      (pointer) => _bindings.command(_session, pointer),
    );

    if (code != AnvilResult.ok) {
      throw AnvilCommandException(code, command['type'] as String);
    }
  }

  // --- event pump -------------------------------------------------------

  Future<void> _startPump() async {
    final port = ReceivePort();
    _pumpPort = port;

    port.listen((message) {
      if (message is! String) return;
      try {
        final json = jsonDecode(message) as Map<String, dynamic>;
        _events.add(AnvilEvent.fromJson(json));
      } catch (_) {
        // A malformed event must not take down the stream the whole UI is
        // listening to.
      }
    });

    _pump = await Isolate.spawn(
      _pumpEntry,
      _PumpArgs(port.sendPort, _session.address),
      debugName: 'anvil-event-pump',
    );
  }

  /// Runs on a background isolate: block on the native queue, forward JSON.
  static void _pumpEntry(_PumpArgs args) {
    final bindings = AnvilBindings.load();
    final session = Pointer<AnvilSession>.fromAddress(args.sessionAddress);

    while (true) {
      final pointer = bindings.nextEvent(session, 250);
      if (pointer == nullptr) {
        continue; // timeout: loop so shutdown is noticed promptly
      }

      try {
        args.sendPort.send(pointer.toDartString());
      } finally {
        bindings.freeString(pointer);
      }
    }
  }

  /// Stop the node and release everything.
  Future<void> dispose() async {
    if (_disposed) return;
    _disposed = true;

    // Kill the pump before shutting the session down, so it cannot call into a
    // freed handle.
    _pump?.kill(priority: Isolate.immediate);
    _pump = null;
    _pumpPort?.close();
    _pumpPort = null;

    try {
      await _platformChannel.invokeMethod<void>('detach');
    } on MissingPluginException {
      // The native session still has to be released in headless tests or on a
      // host that deliberately embeds only the FFI layer.
    }

    _bindings.shutdown(_session);
    _session = nullptr;

    await _events.close();
  }
}

class _PumpArgs {
  const _PumpArgs(this.sendPort, this.sessionAddress);
  final SendPort sendPort;
  final int sessionAddress;
}
