# §99 — Security

Needs a capture host — a laptop on the same network with Wireshark, or an AP that
can mirror traffic.

## 1. Capture

**Steps.** Start a three-person room. Capture traffic at the relay. Talk.

**Expected.** Routing metadata is visible where the design says it should be:
protocol version, packet type, truncated room and sender ids, stream id, sequence,
timestamp, epoch. The Opus payload is **not** recoverable. No key material appears
anywhere in the capture.

**Failure.** Any recognisable audio, any key bytes, or any full peer identity in a
media packet.

## 2. Tamper

**Steps.** With a proxy in the path, flip a bit in a media packet's ciphertext.
Then, separately, alter the sequence number in the header.

**Expected.** Both are discarded at the receiver. The ciphertext change fails the
tag; the header change fails it too, because the header is the associated data.
The audio stream continues — one frame is concealed, nothing more.

**Failure.** Any tampered packet reaching the decoder. Any crash. Any audible
artefact larger than a single concealed frame.

## 3. Replay

**Steps.** Capture media packets. Replay them 30 seconds later, and again after
5 minutes.

**Expected.** All rejected. `packets_rejected_replay` increments in diagnostics.
Nothing is heard twice.

**Failure.** Audio repeating. A counter that does not move — meaning the packets
were dropped for some *other* reason and replay protection is untested.

## 4. Impostor

**Steps.** Have a fourth device advertise a fingerprint and display name copied
from a participant.

**Expected.** It appears in the peer list marked **unverified**. It cannot join
without passing the handshake and admission. No existing participant's name or
entry changes because of its advertisements.

**Failure.** The impostor shown as verified, or a confirmed peer's name changing
to match the impostor's advertisement.

## 5. Departure

**Steps.** Participant D leaves. Continue talking. Confirm D's device can decrypt
nothing sent after departure.

**Expected.** Epoch advanced; D holds no current key material. At most ~500 ms of
already-in-flight audio is still decryptable — the retention window, which exists
so joins and leaves do not glitch the room.

**Failure.** D decrypting anything meaningfully after the rotation.

## 6. Log hygiene

**Steps.** Run with verbose logging and diagnostics on. Search the output.

**Expected.** No private keys, no media keys, no session secrets, no plaintext
audio. Peer and room ids appear only in short form.

**Failure.** Any of the above. This is a straightforward thing to regress and a
serious thing to ship.
