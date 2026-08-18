//! JSON on the FFI boundary.
//!
//! The schema here is the contract with `apps/mobile/lib/services/anvil_api.dart`.
//! Changing a field name here without changing it there produces a silent
//! failure at runtime, so both sides are covered by tests — the ones below and
//! the Dart-side decoding tests.
//!
//! Commands and events use a `type` discriminator with camelCase names,
//! matching Dart convention rather than Rust's, because that is the side that
//! reads them by hand.

use anvil_core::chat::Conversation;
use anvil_core::identity::parse_peer_id;
use anvil_core::peer::{CallEnded, CallState};
use anvil_core::{AnvilConfig, AppState, Command, Event, RoomId};
use serde_json::{json, Value};

/// Parse a config document. Unknown fields are ignored and missing ones take
/// their defaults, so an older host and a newer core still start.
pub fn config_from_json(text: &str) -> Option<AnvilConfig> {
    let value: Value = serde_json::from_str(text).ok()?;
    let mut config = AnvilConfig::default();

    if let Some(name) = value.get("displayName").and_then(Value::as_str) {
        config.display_name = name.to_owned();
    }
    if let Some(on) = value.get("diagnostics").and_then(Value::as_bool) {
        config.diagnostics = on;
    }
    if let Some(on) = value.get("vadEnabled").and_then(Value::as_bool) {
        config.audio.vad_enabled = on;
    }
    if let Some(bps) = value.get("bitrateBps").and_then(Value::as_u64) {
        config.audio.target_bitrate_bps = bps as u32;
    }

    Some(config)
}

/// Parse a command. Returns `None` for anything unrecognised — an unknown
/// command must be a visible error, not a silent no-op.
pub fn command_from_json(text: &str) -> Option<Command> {
    let value: Value = serde_json::from_str(text).ok()?;

    Some(match value.get("type")?.as_str()? {
        // --- identity ---------------------------------------------------
        "createProfile" => Command::CreateProfile {
            display_name: value.get("displayName")?.as_str()?.to_owned(),
        },
        "renameProfile" => Command::RenameProfile {
            display_name: value.get("displayName")?.as_str()?.to_owned(),
        },
        "verifyPeer" => Command::VerifyPeer {
            peer_id: parse_peer_id(value.get("peerId")?.as_str()?)?,
        },
        "acceptIdentityChange" => Command::AcceptIdentityChange {
            peer_id: parse_peer_id(value.get("peerId")?.as_str()?)?,
        },

        // --- calls ------------------------------------------------------
        "callPeer" => Command::CallPeer {
            peer_id: parse_peer_id(value.get("peerId")?.as_str()?)?,
        },
        "acceptCall" => Command::AcceptCall,
        "declineCall" => Command::DeclineCall,
        "endCall" => Command::EndCall,

        // --- chat -------------------------------------------------------
        "sendMessage" => Command::SendMessage {
            conversation: conversation_from_json(&value)?,
            body: value.get("body")?.as_str()?.to_owned(),
        },

        "joinRoomByCode" => Command::JoinRoomByCode {
            code: value.get("code")?.as_str()?.to_owned(),
        },

        "startDiscovery" => Command::StartDiscovery,
        "stopDiscovery" => Command::StopDiscovery,
        "createRoom" => Command::CreateRoom,
        "joinRoom" => Command::JoinRoom {
            room_id: room_id_from_hex(value.get("roomId")?.as_str()?)?,
            credential: value
                .get("credential")
                .and_then(Value::as_str)
                .map(|c| c.as_bytes().to_vec()),
        },
        "leaveRoom" => Command::LeaveRoom,
        "mute" => Command::Mute,
        "unmute" => Command::Unmute,
        "requestDiagnostics" => Command::RequestDiagnostics,
        _ => return None,
    })
}

/// Render an event.
pub fn event_to_json(event: &Event) -> String {
    let value = match event {
        Event::StateChanged { state } => json!({
            "type": "stateChanged",
            "state": state_name(*state),
        }),
        Event::PeerDiscovered { peer } => json!({
            "type": "peerDiscovered",
            "fingerprint": hex(&peer.fingerprint),
            "displayName": peer.display_name,
            "confirmed": peer.confirmed,
            "hostingRoom": peer.room_hint.is_some(),
            "transports": peer.kinds().iter().map(ToString::to_string).collect::<Vec<_>>(),
        }),
        Event::PeerLost { peer_id } => json!({
            "type": "peerLost",
            "peerId": peer_id.short(),
        }),
        Event::ProfileReady { profile } => json!({
            "type": "profileReady",
            "displayName": profile.display_name,
            "peerId": anvil_core::identity::peer_id_string(profile.peer_id),
            "shortPeerId": anvil_core::identity::peer_id_short_string(profile.peer_id),
            "fingerprint": profile.fingerprint().short(),
            "fingerprintLong": profile.fingerprint().long(),
            "protocolVersion": profile.protocol_version,
        }),

        Event::IdentityChanged {
            peer_id,
            display_name,
            previous_fingerprint,
            new_fingerprint,
        } => json!({
            "type": "identityChanged",
            "peerId": anvil_core::identity::peer_id_string(*peer_id),
            "displayName": display_name,
            "previousFingerprint": previous_fingerprint.short(),
            "newFingerprint": new_fingerprint.short(),
        }),

        Event::PeerRenamed { peer_id, previous_name, display_name } => json!({
            "type": "peerRenamed",
            "peerId": anvil_core::identity::peer_id_string(*peer_id),
            "previousName": previous_name,
            "displayName": display_name,
        }),

        Event::IncomingCall { peer_id, display_name } => json!({
            "type": "incomingCall",
            "peerId": anvil_core::identity::peer_id_string(*peer_id),
            "displayName": display_name,
        }),

        Event::CallStateChanged { state, display_name } => json!({
            "type": "callStateChanged",
            "call": call_state_name(*state),
            "peerId": state.peer().map(anvil_core::identity::peer_id_string),
            "displayName": display_name,
        }),

        Event::CallFinished { peer_id, reason } => json!({
            "type": "callFinished",
            "peerId": peer_id.map(anvil_core::identity::peer_id_string),
            "reason": call_ended_name(*reason),
        }),

        Event::MessageReceived { message } => json!({
            "type": "messageReceived",
            "id": message.id.to_hex(),
            "from": anvil_core::identity::peer_id_string(message.from),
            "conversation": conversation_to_json(message.conversation),
            "body": message.body,
            "atMs": message.at.as_millis(),
            "outbound": message.outbound,
            "delivery": delivery_to_json(message.delivery),
        }),

        Event::MessageDelivery { id, delivery } => json!({
            "type": "messageDelivery",
            "id": id.to_hex(),
            "delivery": delivery_to_json(*delivery),
        }),

        Event::RoomCreated { room_id, join_code } => json!({
            "type": "roomCreated",
            "roomId": hex(room_id.as_bytes()),
            "shortId": room_id.short(),
            "joinCode": join_code.formatted(),
        }),
        Event::RoomJoined { room } => json!({
            "type": "roomJoined",
            "roomId": hex(room.room_id.as_bytes()),
            "shortId": room.room_id.short(),
            "epoch": room.epoch.0,
            "isHost": room.is_host,
            "isDirect": room.is_direct,
            "relay": room.relay.map(|r| r.short()),
            "participants": room.participants.iter().map(|p| json!({
                "peerId": p.peer_id.short(),
                "displayName": p.display_name,
                "speaking": p.speaking,
                "muted": p.muted,
            })).collect::<Vec<_>>(),
        }),
        Event::RoomLeft { room_id, reason } => json!({
            "type": "roomLeft",
            "roomId": hex(room_id.as_bytes()),
            "reason": reason,
        }),
        Event::ParticipantJoined { participant } => json!({
            "type": "participantJoined",
            "peerId": participant.peer_id.short(),
            "displayName": participant.display_name,
        }),
        Event::ParticipantLeft { peer_id } => json!({
            "type": "participantLeft",
            "peerId": peer_id.short(),
        }),
        Event::SpeakingChanged { peer_id, speaking } => json!({
            "type": "speakingChanged",
            "peerId": peer_id.short(),
            "speaking": speaking,
        }),
        Event::TransportChanged { peer_id, active, standby } => json!({
            "type": "transportChanged",
            "peerId": peer_id.short(),
            "active": active.to_string(),
            "standby": standby.map(|s| s.to_string()),
        }),
        Event::RelayChanged { relay, reason } => json!({
            "type": "relayChanged",
            "relay": relay.map(|r| r.short()),
            "reason": reason,
        }),
        Event::ConnectionDegraded { peer_id, quality } => json!({
            "type": "connectionDegraded",
            "peerId": peer_id.map(|p| p.short()),
            "quality": format!("{quality:?}").to_lowercase(),
        }),
        Event::ConnectionRecovered { peer_id } => json!({
            "type": "connectionRecovered",
            "peerId": peer_id.map(|p| p.short()),
        }),
        Event::MuteChanged { muted } => json!({
            "type": "muteChanged",
            "muted": muted,
        }),
        Event::EpochChanged { epoch } => json!({
            "type": "epochChanged",
            "epoch": epoch.0,
        }),
        Event::PermissionRequired { capability } => json!({
            "type": "permissionRequired",
            "capability": capability,
        }),
        Event::Diagnostics { snapshot } => json!({
            "type": "diagnostics",
            "localPeer": snapshot.local_peer,
            "room": snapshot.room,
            "relay": snapshot.relay,
            "isRelay": snapshot.is_relay,
            "epoch": snapshot.epoch.0,
            "participants": snapshot.participant_count,
            "opusBitrateBps": snapshot.opus_bitrate_bps,
            "packetsSent": snapshot.counters.packets_sent,
            "packetsReceived": snapshot.counters.packets_received,
            "packetsRejectedAuth": snapshot.counters.packets_rejected_auth,
            "packetsRejectedReplay": snapshot.counters.packets_rejected_replay,
            "framesConcealed": snapshot.counters.frames_concealed,
            "pathSwitches": snapshot.counters.path_switches,
            "relayChanges": snapshot.counters.relay_changes,
            "paths": snapshot.paths.iter().map(|p| json!({
                "kind": p.kind.to_string(),
                "rttMs": p.rtt.as_millis(),
                "loss": p.loss,
                "jitterMs": p.jitter.as_millis(),
                "score": p.score,
                "active": p.active,
            })).collect::<Vec<_>>(),
        }),
        Event::Error { layer, message } => json!({
            "type": "error",
            "layer": layer,
            "message": message,
        }),
        // The event enum is #[non_exhaustive]; a new variant should surface as
        // a visible unknown rather than vanish.
        _ => json!({ "type": "unknown" }),
    };

    value.to_string()
}

/// Parse a conversation reference: either a peer or a room.
fn conversation_from_json(value: &Value) -> Option<Conversation> {
    if let Some(peer) = value.get("peerId").and_then(Value::as_str) {
        return Some(Conversation::Direct(parse_peer_id(peer)?));
    }
    let room = value.get("roomId").and_then(Value::as_str)?;
    Some(Conversation::Room(room_id_from_hex(room)?))
}

fn conversation_to_json(conversation: Conversation) -> Value {
    match conversation {
        Conversation::Direct(peer) => json!({
            "kind": "direct",
            "peerId": anvil_core::identity::peer_id_string(peer),
        }),
        Conversation::Room(room) => json!({
            "kind": "room",
            "roomId": hex(room.as_bytes()),
        }),
    }
}

fn delivery_to_json(delivery: anvil_core::chat::DeliveryState) -> Value {
    use anvil_core::chat::DeliveryState as D;

    match delivery {
        D::Pending => json!({ "state": "pending" }),
        D::Sent => json!({ "state": "sent" }),
        D::Delivered => json!({ "state": "delivered" }),
        D::Undeliverable => json!({ "state": "undeliverable" }),
        D::Partial { delivered, total } => json!({
            "state": "partial",
            "delivered": delivered,
            "total": total,
        }),
    }
}

fn call_state_name(state: CallState) -> &'static str {
    match state {
        CallState::Idle => "idle",
        CallState::Outgoing { .. } => "outgoing",
        CallState::Incoming { .. } => "incoming",
        CallState::Active { .. } => "active",
    }
}

fn call_ended_name(reason: CallEnded) -> &'static str {
    match reason {
        CallEnded::Cancelled => "cancelled",
        CallEnded::Declined => "declined",
        CallEnded::HungUp => "hungUp",
        CallEnded::Unanswered => "unanswered",
        CallEnded::Unreachable => "unreachable",
    }
}

fn state_name(state: AppState) -> &'static str {
    match state {
        AppState::Initializing => "initializing",
        AppState::PermissionsRequired => "permissionsRequired",
        AppState::Idle => "idle",
        AppState::Discovering => "discovering",
        AppState::CreatingRoom => "creatingRoom",
        AppState::JoiningRoom => "joiningRoom",
        AppState::Connected => "connected",
        AppState::Reconnecting => "reconnecting",
        AppState::RelayElection => "relayElection",
        AppState::Leaving => "leaving",
        AppState::Error => "error",
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn room_id_from_hex(text: &str) -> Option<RoomId> {
    if text.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(RoomId(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_simple_command_parses() {
        for (text, matches) in [
            (r#"{"type":"startDiscovery"}"#, matches!(
                command_from_json(r#"{"type":"startDiscovery"}"#),
                Some(Command::StartDiscovery)
            )),
            (r#"{"type":"createRoom"}"#, matches!(
                command_from_json(r#"{"type":"createRoom"}"#),
                Some(Command::CreateRoom)
            )),
            (r#"{"type":"leaveRoom"}"#, matches!(
                command_from_json(r#"{"type":"leaveRoom"}"#),
                Some(Command::LeaveRoom)
            )),
            (r#"{"type":"mute"}"#, matches!(
                command_from_json(r#"{"type":"mute"}"#),
                Some(Command::Mute)
            )),
        ] {
            assert!(matches, "failed to parse {text}");
        }
    }

    #[test]
    fn join_room_round_trips_a_room_id() {
        let room = RoomId::generate();
        let text = format!(r#"{{"type":"joinRoom","roomId":"{}"}}"#, hex(room.as_bytes()));

        match command_from_json(&text) {
            Some(Command::JoinRoom { room_id, credential }) => {
                assert_eq!(room_id, room);
                assert!(credential.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn join_room_carries_a_credential_when_present() {
        let room = RoomId::generate();
        let text = format!(
            r#"{{"type":"joinRoom","roomId":"{}","credential":"742913"}}"#,
            hex(room.as_bytes())
        );

        match command_from_json(&text) {
            Some(Command::JoinRoom { credential: Some(c), .. }) => {
                assert_eq!(c, b"742913".to_vec());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn malformed_input_is_rejected_rather_than_defaulted() {
        for text in [
            "",
            "{",
            "[]",
            r#"{"type":42}"#,
            r#"{"type":"nope"}"#,
            r#"{"type":"joinRoom"}"#,
            r#"{"type":"joinRoom","roomId":"nothex"}"#,
            r#"{"type":"joinRoom","roomId":"zz00000000000000000000000000000000"}"#,
        ] {
            assert!(command_from_json(text).is_none(), "accepted {text:?}");
        }
    }

    #[test]
    fn events_render_with_a_type_discriminator() {
        let json = event_to_json(&Event::StateChanged { state: AppState::Connected });
        let value: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["type"], "stateChanged");
        assert_eq!(value["state"], "connected");
    }

    #[test]
    fn room_created_carries_the_ids_and_the_code_to_read_aloud() {
        let room = RoomId::generate();
        let join_code = anvil_core::room::JoinCode::generate();
        let json = event_to_json(&Event::RoomCreated { room_id: room, join_code });
        let value: Value = serde_json::from_str(&json).unwrap();

        // Full id so the host can pass it back in joinRoom...
        assert_eq!(value["roomId"], hex(room.as_bytes()));
        // ...short id for display...
        assert_eq!(value["shortId"], room.short());
        // ...and the join code, which is the thing a person actually shares.
        assert_eq!(value["joinCode"], join_code.formatted());
        assert!(value["joinCode"].as_str().unwrap().starts_with("ANV-"));
    }

    #[test]
    fn identity_commands_round_trip_a_full_peer_id() {
        let peer = anvil_core::PeerId([0x5A; 32]);
        let text = format!(
            r#"{{"type":"callPeer","peerId":"{}"}}"#,
            anvil_core::identity::peer_id_string(peer)
        );

        match command_from_json(&text) {
            Some(Command::CallPeer { peer_id }) => assert_eq!(peer_id, peer),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_truncated_peer_id_is_refused_rather_than_guessed() {
        // The short form is lossy by design; accepting it here would make it a
        // de facto identifier.
        let text = r#"{"type":"callPeer","peerId":"anv_5a5a5…"}"#;
        assert!(command_from_json(text).is_none());
    }

    #[test]
    fn first_run_is_a_single_command_with_no_credentials() {
        let text = r#"{"type":"createProfile","displayName":"Femi"}"#;

        match command_from_json(text) {
            Some(Command::CreateProfile { display_name }) => {
                assert_eq!(display_name, "Femi");
            }
            other => panic!("{other:?}"),
        }

        // There is nothing else to supply — no password, no email, no server.
        assert!(command_from_json(r#"{"type":"createProfile"}"#).is_none());
    }

    #[test]
    fn messages_can_be_addressed_to_a_peer_or_a_room() {
        let peer = anvil_core::PeerId([3; 32]);
        let direct = format!(
            r#"{{"type":"sendMessage","peerId":"{}","body":"Hey"}}"#,
            anvil_core::identity::peer_id_string(peer)
        );
        match command_from_json(&direct) {
            Some(Command::SendMessage { conversation, body }) => {
                assert_eq!(conversation, Conversation::Direct(peer));
                assert_eq!(body, "Hey");
            }
            other => panic!("{other:?}"),
        }

        let room = RoomId::generate();
        let to_room = format!(
            r#"{{"type":"sendMessage","roomId":"{}","body":"Hi all"}}"#,
            hex(room.as_bytes())
        );
        match command_from_json(&to_room) {
            Some(Command::SendMessage { conversation, .. }) => {
                assert_eq!(conversation, Conversation::Room(room));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn join_by_code_passes_the_raw_string_through_for_forgiving_parsing() {
        // Normalisation belongs in the core, so every host gets the same
        // leniency rather than each reimplementing it.
        match command_from_json(r#"{"type":"joinRoomByCode","code":"anv 7fk2 p9w4"}"#) {
            Some(Command::JoinRoomByCode { code }) => assert_eq!(code, "anv 7fk2 p9w4"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn call_states_and_end_reasons_have_distinct_names() {
        let peer = anvil_core::PeerId([1; 32]);
        let at = anvil_core::Monotonic(0);

        let states = [
            CallState::Idle,
            CallState::Outgoing { peer, since: at },
            CallState::Incoming { peer, since: at },
            CallState::Active { peer, since: at },
        ];
        let names: std::collections::HashSet<_> =
            states.iter().map(|s| call_state_name(*s)).collect();
        assert_eq!(names.len(), states.len());

        let reasons = [
            CallEnded::Cancelled,
            CallEnded::Declined,
            CallEnded::HungUp,
            CallEnded::Unanswered,
            CallEnded::Unreachable,
        ];
        let names: std::collections::HashSet<_> =
            reasons.iter().map(|r| call_ended_name(*r)).collect();
        assert_eq!(names.len(), reasons.len());
    }

    #[test]
    fn every_app_state_has_a_distinct_name() {
        let states = [
            AppState::Initializing,
            AppState::PermissionsRequired,
            AppState::Idle,
            AppState::Discovering,
            AppState::CreatingRoom,
            AppState::JoiningRoom,
            AppState::Connected,
            AppState::Reconnecting,
            AppState::RelayElection,
            AppState::Leaving,
            AppState::Error,
        ];

        let names: std::collections::HashSet<_> = states.iter().map(|s| state_name(*s)).collect();
        assert_eq!(names.len(), states.len());
    }

    #[test]
    fn config_parsing_ignores_unknown_fields() {
        let config = config_from_json(
            r#"{"displayName":"Adeleke","diagnostics":true,"somethingNew":123}"#,
        )
        .expect("should parse");

        assert_eq!(config.display_name, "Adeleke");
        assert!(config.diagnostics);
    }

    #[test]
    fn config_parsing_rejects_non_json() {
        assert!(config_from_json("not json").is_none());
    }

    #[test]
    fn error_events_name_the_layer() {
        let json =
            event_to_json(&Event::Error { layer: "transport", message: "no path".into() });
        let value: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["layer"], "transport");
        assert_eq!(value["message"], "no path");
    }
}
