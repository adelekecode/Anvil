//! Session-encrypted application control carried over reliable QUIC streams.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use zeroize::ZeroizeOnDrop;

use crate::chat::MessageId;
use crate::{CryptoError, ProtocolError, Result};

const MAGIC: &[u8; 4] = b"CTL1";

/// Live application actions exchanged after identity authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppControl {
    /// Ask the authenticated peer to ring.
    CallRequest,
    /// Answer a pending call.
    CallAccept,
    /// Decline a pending call.
    CallDecline,
    /// End a ringing or active call.
    CallEnd,
    /// Live direct chat message.
    Chat {
        /// Sender-generated message id.
        id: MessageId,
        /// UTF-8 body.
        body: String,
    },
    /// Application-level receipt for a chat message.
    ChatAck {
        /// Acknowledged id.
        id: MessageId,
    },
    /// Present a human-entered room code to an authenticated nearby host.
    RoomJoin {
        /// Normalized displayed code.
        code: String,
    },
    /// Host admitted this peer and supplied the stable room identity.
    RoomAccept {
        /// Full room id.
        room_id: [u8; 16],
        /// Membership epoch after admission.
        epoch: u64,
    },
    /// Deliver this authenticated sender's media key for one room epoch.
    MediaKey {
        /// Room membership epoch.
        epoch: u64,
        /// ChaCha20-Poly1305 key.
        key: [u8; 32],
        /// Per-key nonce salt.
        salt: [u8; 12],
    },
}

/// Per-path ordered encryption state. Sequence numbers are never reused.
#[derive(ZeroizeOnDrop)]
pub struct SecureControl {
    key: [u8; 32],
    send_sequence: u64,
    receive_sequence: u64,
}

impl core::fmt::Debug for SecureControl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SecureControl")
            .field("send_sequence", &self.send_sequence)
            .field("receive_sequence", &self.receive_sequence)
            .finish_non_exhaustive()
    }
}

impl SecureControl {
    /// Install the key derived by the X25519 handshake.
    #[must_use]
    pub const fn new(key: [u8; 32]) -> Self {
        Self { key, send_sequence: 0, receive_sequence: 0 }
    }

    /// Encrypt one control record with its sequence bound as associated data.
    pub fn seal(&mut self, message: &AppControl) -> Result<Vec<u8>> {
        let sequence = self.send_sequence;
        self.send_sequence = self
            .send_sequence
            .checked_add(1)
            .ok_or(CryptoError::HandshakeFailed("control sequence exhausted"))?;
        let plaintext = encode(message)?;
        let mut associated = [0u8; 12];
        associated[..4].copy_from_slice(MAGIC);
        associated[4..].copy_from_slice(&sequence.to_be_bytes());
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce(sequence)),
                Payload { msg: &plaintext, aad: &associated },
            )
            .map_err(|_| CryptoError::AuthenticationFailed)?;
        let mut out = associated.to_vec();
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Authenticate, order-check and decode one control record.
    pub fn open(&mut self, record: &[u8]) -> Result<AppControl> {
        if record.len() < 12 + 16 || &record[..4] != MAGIC {
            return Err(ProtocolError::Malformed("invalid encrypted control record").into());
        }
        let sequence = u64::from_be_bytes(record[4..12].try_into().expect("checked length"));
        if sequence != self.receive_sequence {
            return Err(CryptoError::ReplayRejected.into());
        }
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce(sequence)),
                Payload { msg: &record[12..], aad: &record[..12] },
            )
            .map_err(|_| CryptoError::AuthenticationFailed)?;
        let message = decode(&plaintext)?;
        self.receive_sequence += 1;
        Ok(message)
    }

    /// Whether bytes are an encrypted application record.
    #[must_use]
    pub fn is_record(record: &[u8]) -> bool {
        record.starts_with(MAGIC)
    }
}

fn nonce(sequence: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(b"CTRL");
    nonce[4..].copy_from_slice(&sequence.to_be_bytes());
    nonce
}

fn encode(message: &AppControl) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match message {
        AppControl::CallRequest => out.push(1),
        AppControl::CallAccept => out.push(2),
        AppControl::CallDecline => out.push(3),
        AppControl::CallEnd => out.push(4),
        AppControl::Chat { id, body } => {
            let body = body.as_bytes();
            let length = u16::try_from(body.len())
                .map_err(|_| ProtocolError::Malformed("chat body too large"))?;
            out.push(5);
            out.extend_from_slice(&id.0);
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(body);
        }
        AppControl::ChatAck { id } => {
            out.push(6);
            out.extend_from_slice(&id.0);
        }
        AppControl::RoomJoin { code } => {
            let code = code.as_bytes();
            let length = u8::try_from(code.len())
                .map_err(|_| ProtocolError::Malformed("room code too long"))?;
            out.push(7);
            out.push(length);
            out.extend_from_slice(code);
        }
        AppControl::RoomAccept { room_id, epoch } => {
            out.push(8);
            out.extend_from_slice(room_id);
            out.extend_from_slice(&epoch.to_be_bytes());
        }
        AppControl::MediaKey { epoch, key, salt } => {
            out.push(9);
            out.extend_from_slice(&epoch.to_be_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(salt);
        }
    }
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<AppControl> {
    let Some(kind) = bytes.first().copied() else {
        return Err(ProtocolError::Malformed("empty application control").into());
    };
    Ok(match kind {
        1 if bytes.len() == 1 => AppControl::CallRequest,
        2 if bytes.len() == 1 => AppControl::CallAccept,
        3 if bytes.len() == 1 => AppControl::CallDecline,
        4 if bytes.len() == 1 => AppControl::CallEnd,
        5 if bytes.len() >= 15 => {
            let mut id = [0u8; 12];
            id.copy_from_slice(&bytes[1..13]);
            let length = u16::from_be_bytes([bytes[13], bytes[14]]) as usize;
            if bytes.len() != 15 + length {
                return Err(ProtocolError::Malformed("invalid chat length").into());
            }
            let body = std::str::from_utf8(&bytes[15..])
                .map_err(|_| ProtocolError::Malformed("chat is not UTF-8"))?
                .to_owned();
            AppControl::Chat { id: MessageId(id), body }
        }
        6 if bytes.len() == 13 => {
            let mut id = [0u8; 12];
            id.copy_from_slice(&bytes[1..]);
            AppControl::ChatAck { id: MessageId(id) }
        }
        7 if bytes.len() >= 2 => {
            let length = bytes[1] as usize;
            if bytes.len() != 2 + length {
                return Err(ProtocolError::Malformed("invalid room code length").into());
            }
            let code = std::str::from_utf8(&bytes[2..])
                .map_err(|_| ProtocolError::Malformed("room code is not UTF-8"))?
                .to_owned();
            AppControl::RoomJoin { code }
        }
        8 if bytes.len() == 25 => {
            let mut room_id = [0u8; 16];
            room_id.copy_from_slice(&bytes[1..17]);
            let epoch = u64::from_be_bytes(bytes[17..25].try_into().expect("checked length"));
            AppControl::RoomAccept { room_id, epoch }
        }
        9 if bytes.len() == 53 => {
            let epoch = u64::from_be_bytes(bytes[1..9].try_into().expect("checked length"));
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes[9..41]);
            let mut salt = [0u8; 12];
            salt.copy_from_slice(&bytes[41..53]);
            AppControl::MediaKey { epoch, key, salt }
        }
        _ => return Err(ProtocolError::Malformed("unknown application control").into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_is_encrypted_and_ordered() {
        let mut sender = SecureControl::new([7; 32]);
        let mut receiver = SecureControl::new([7; 32]);
        let record = sender.seal(&AppControl::CallRequest).unwrap();
        assert!(record.len() > 1);
        assert_eq!(receiver.open(&record).unwrap(), AppControl::CallRequest);
        assert!(receiver.open(&record).is_err());
    }

    #[test]
    fn chat_round_trips() {
        let id = MessageId([3; 12]);
        let message = AppControl::Chat { id, body: "hello".into() };
        let mut sender = SecureControl::new([9; 32]);
        let mut receiver = SecureControl::new([9; 32]);
        assert_eq!(receiver.open(&sender.seal(&message).unwrap()).unwrap(), message);
    }
}
