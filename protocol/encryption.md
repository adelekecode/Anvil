# Encryption

## 1. Two layers, two adversaries

```
 ┌────────────────────────────────────────────────┐
 │ End-to-end media encryption                    │
 │   against: the relay, and anyone on any hop    │
 │   terminated at: the two endpoints             │
 ├────────────────────────────────────────────────┤
 │ Transport security (QUIC/TLS)                  │
 │   against: attackers on one hop                │
 │   terminated at: each hop, including the relay │
 └────────────────────────────────────────────────┘
```

Confusing these is the most common way a system turns out not to be end-to-end
encrypted. **The relay terminates QUIC.** If media were protected only by
transport security, the relay would see plaintext. It does not, because media is
sealed before it is handed to the transport and opened only by endpoints holding
the sender's key.

## 2. Primitives

No invented cryptography. Established primitives via reviewed Rust crates.

| Purpose | Primitive |
|---|---|
| Device identity, signatures | Ed25519 |
| Session key agreement | X25519 |
| Key derivation | HKDF-SHA-256 |
| Media encryption | ChaCha20-Poly1305 |
| Peer id derivation | SHA-256 of the identity public key |

## 3. Identity

Each installation generates one long-lived Ed25519 keypair on first run. The
private half never leaves the device, and where the hardware allows, never
leaves secure storage.

`PeerId = SHA-256(identity public key)`, so ids are fixed-width and
scheme-agnostic — changing the primitive later changes how ids are computed, not
what they are.

**What a verified identity means:** the same device as last time. **What it does
not mean:** anything about who is holding it. Anvil has no directory, no
accounts, and no authority to ask. Trust is established out of band — the
participants are in the same room and can see each other — and the UI should
reflect that rather than implying a verified name is a verified person.

### 3.1 Storage

iOS Keychain with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`; Android
Keystore, StrongBox or TEE where available.

Two awkward realities worth planning for rather than discovering:

- The Secure Enclave supports P-256, not Ed25519, so hardware protection means
  *wrapping* the Ed25519 key with an Enclave key, not generating it inside.
- Android Keystore's Ed25519 support is uneven across vendors and API levels.

Either way, `Capabilities::secure_key_storage` must report what is actually in
use so the diagnostics view does not overstate the guarantee.

**The identity must not sync or restore.** A restored identity means two phones
claiming to be the same peer, which the protocol cannot resolve.

## 4. Handshake

```
 Hello       ──►  version check, fingerprint
 Identity    ◄─►  Ed25519 key + X25519 ephemeral + signature over the transcript
 ─────────────    session keys via HKDF-SHA-256
 RoomJoin    ──►  admission
 KeyExchange ◄─►  per-sender media keys for the current epoch
```

Three properties, none optional:

**Ephemeral session keys, separate from identity.** A compromised identity key
must not decrypt yesterday's recorded call. X25519 ephemerals give forward
secrecy for the session; the identity key only authenticates.

**Transcript binding.** The signature covers both sides' nonces and the
negotiated version, so a captured handshake cannot be replayed against a
different session and a version downgrade cannot be forced by rewriting a byte.
*The exact transcript definition is unpinned and must be written down before it
is implemented* — getting it wrong is precisely a downgrade or replay
vulnerability.

**Rejection is terminal.** A failed handshake tears the path down. There is no
retry-with-less: an attacker who can make verification fail should get a closed
connection, not a weaker one.

## 5. Group keys: the v0.1 choice

Each participant encrypts their own voice with their own key, distributed to
every other authorised member over authenticated peer sessions.

```
 Alice ── key_A ──► Bob, Chris, David
 Bob   ── key_B ──► Alice, Chris, David
 Chris ── key_C ──► Alice, Bob, David
```

### 5.1 What this buys

**Sender authentication for free.** Only Alice holds Alice's key, so a packet
that opens under key_A was authored by Alice. A single shared room key would let
any member forge any other member's voice — including a malicious relay who is
also a participant, which is exactly the adversary the threat model names.

### 5.2 What it costs

**O(n²) key deliveries per epoch, and every membership change forces an epoch.**

| Room size | Deliveries per membership change |
|---|---|
| 3 | 6 |
| 4 | 12 |
| 8 | 56 |
| 12 | 132 |

Fine for the three-to-four-person rooms v0.1 targets. Not fine beyond that, over
a shared radio, on every join and leave. **This scheme will need replacing before
rooms get meaningfully larger**, and that is the MLS work v0.1 defers.

### 5.3 The MLS seam

Everything above the crypto module talks to `GroupKeyManager` — seal a frame for
the room, open a frame from a member, advance on membership change — never to
sender-key internals. Swapping in MLS should touch this module and nothing else.
That abstraction is the entire reason deferring MLS is a reasonable decision
rather than a debt that compounds.

## 6. Nonces

ChaCha20-Poly1305 takes a 96-bit nonce, and **reusing one under the same key is
fatal** — it leaks the XOR of two plaintexts and permits forgery. Anvil derives
it rather than randomising it:

```
 nonce = salt(12, per-key, from HKDF) XOR (epoch:4 ‖ stream:2 ‖ sequence:4 ‖ 0:2)
```

Uniqueness follows structurally: the sender advances the sequence monotonically,
and the key changes on every epoch. Random nonces would risk collision after
~2⁴⁸ frames; derived nonces make it impossible. It also saves 12 bytes per packet
— the receiver reconstructs the nonce from the header.

**Sequence numbering restarts at each epoch.** That is safe precisely because the
epoch is in the nonce, and it keeps the sequence space small.

## 7. Replay rejection

An attacker who captures a packet can resend it. Without a check, the receiver
would decrypt it happily — valid tag, real packet — and the user hears a word
twice, or a "yes" from ten minutes ago.

A 64-packet sliding window per (sender, stream, epoch), IPSec/DTLS style:

- newer than the highest seen → accept, shift the window;
- inside the window and not yet seen → accept, mark it;
- otherwise → reject.

A window rather than a counter, because real networks reorder and rejecting all
out-of-order packets would discard audio the jitter buffer could have used.
64 packets is ~1.3 s at 20 ms frames — wider than any local reordering, narrower
than the jitter buffer's tolerance, so anything rejected here was too old to play.

**The window is updated only after the tag verifies.** Recording an
unauthenticated sequence number would let an attacker who cannot decrypt anything
still censor the stream by pre-claiming sequence numbers.

## 8. Epochs and the retention window

Every membership change advances the epoch and produces new key material. That is
what makes departure mean something: after the rotation, the departed member
holds keys for a generation nobody uses.

But rotation cannot be instantaneous. When David leaves at epoch 41→42, packets
under epoch 41 are still in flight — a couple of hundred milliseconds on a jittery
path. Discarding epoch 41 immediately drops audio legitimately sent by legitimate
members, and the room audibly glitches on every join and leave.

So superseded keys are retained for **500 ms**, and the trade-off is explicit:

- too short → audible dropouts on every membership change;
- too long → a departed member decrypts for that much longer.

500 ms comfortably covers the maximum jitter buffer depth plus a path switch, and
is far shorter than the time it takes someone to walk out of Wi-Fi range. Note
the bound is on *already-sent* traffic only: a departed member receives no new key
material, so they cannot follow the conversation forward under any setting.

Retention must be enforced on a timer, not lazily on the next packet — retention
that only expires when traffic arrives lasts until the room goes quiet, which is
exactly when it matters.

## 9. Order of operations on receive

Strict, and enforced in that order:

```
1. parse           ← the only step that touches attacker bytes without a key
2. known epoch?    ← reject before any cryptographic work
3. authenticate    ← AEAD tag
4. replay check    ← only now update the window
5. decrypt
6. decode          ← Opus, a large C surface, sees only member-authored bytes
```

Step 6 is why step 3 cannot move. A packet that fails authentication must never
reach the decoder.

## 10. What encryption does not prevent

An authorised participant can record what they legitimately hear, or repeat it.
End-to-end encryption protects data in transit from unauthorised intermediaries;
it cannot stop a human recipient from remembering. The UI should not imply
otherwise.

A malicious relay can drop, delay, reorder and refuse to forward packets, and can
observe metadata: that a participant transmitted, roughly how much, and when. It
cannot decrypt, cannot modify authenticated media undetected, cannot forge
another participant's voice, and cannot derive future keys from its position.

## 11. Never logged

Private keys, media keys, session secrets, plaintext audio. The diagnostics
snapshot type has no field capable of holding any of them — a structural
guarantee rather than a discipline.
