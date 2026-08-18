# Identity

How Anvil knows who is who, with no accounts, no server and no authority.

## 1. The model

```
  Install
     ↓
  "Display name: [ Femi ]"        ← the only thing ever asked for
     ↓
  generate Ed25519 keypair on the device
     ↓
  store the private half in platform secure storage
     ↓
  ready
```

There is no signup, no login, no password, no email, no phone number, no OAuth,
no session server and no user database. Subsequent launches load the local
profile and start discovery; there is no screen between opening the app and
using it.

| Conventional | Anvil |
|---|---|
| account | a keypair |
| user id | `PeerId` = SHA-256(public identity key) |
| login | loading the local profile |
| session token | nothing |
| password reset | **nothing — this is a real cost, see §6** |

## 2. Two names for a person

| | Display name | `PeerId` |
|---|---|---|
| For | humans | the protocol |
| Unique | **no** | yes |
| Chosen by | the user | the key |
| Trustworthy | only once verified | inherently |

Two people can both be "Femi". Anvil knows them as `anv_a82…` and `anv_c93…`.
Conflating the two — treating a name as an identity anywhere in the protocol —
is the mistake this split exists to prevent, and it is why the peer list marks
unconfirmed names rather than presenting them as people.

### 2.1 Displayed forms

| Form | Example | Used for |
|---|---|---|
| Full | `anv_` + 64 hex | verification, diagnostics, anything exact |
| Short | `anv_7ab93…` | showing *an* identity without claiming to show all of it |
| Fingerprint | `7A:42:19:BC` | a human reading it aloud |
| Long fingerprint | `7A42 19BC 3F08 …` | careful in-person comparison |

The short form ends in an ellipsis and **deliberately does not parse back**. If
it round-tripped it would become a de facto identifier and people would start
comparing them.

## 3. Fingerprints and their limits

Four bytes is 32 bits. Generating a second identity whose fingerprint collides
with a given one costs roughly 2³² key generations — hours of ordinary compute.
So a 4-byte fingerprint is **not** sufficient against a determined, targeted
attacker.

It is sufficient for what it is used for: confirming the Daniel in front of you
is the Daniel your phone remembers, against a nearby opportunistic attacker who
did not know in advance whose identity they wanted to collide with. And a
fingerprint nobody reads because it is 64 characters long protects nothing.

Consequently:

* **QR verification carries the full public key**, not the fingerprint. Anything
  security-critical uses the whole key.
* A long form exists for people who want to compare more.

## 4. Trust on first use

```
  first meeting        →  record the key           "Daniel — new"
  same key again       →  recognise it             "Daniel ★"
  same name, new key   →  WARN                     "Daniel's identity changed"
```

### 4.1 What it defends against

Display names are free; anyone in radio range can advertise "Daniel". Without
TOFU, a stranger typing the right name is indistinguishable from the person you
spoke to yesterday. With it, an impostor is *a new peer who happens to share a
name* — which the UI shows plainly — and the more dangerous case, substituting a
new key for a name you already trust, produces an explicit warning.

### 4.2 What it does not defend against

**The first meeting.** If the very first "Daniel" you meet is an impostor, TOFU
faithfully remembers the impostor. That is inherent to the model, and it is why
out-of-band verification exists as a second step: it upgrades a peer from "same
as last time" to "confirmed by a human".

The honest summary for the UI: **unverified means unverified**, not "probably
fine".

### 4.3 The warning has to be worded carefully

The overwhelmingly common cause of a changed identity is a reinstall. Leading
with "someone is impersonating Daniel" would cry wolf, and users who are cried
wolf at learn to tap through warnings — at which point the warning is worse than
useless, because it has trained the behaviour that defeats it.

So the sheet gives both explanations equal weight, says plainly that Anvil
cannot tell which, and makes the safe action ("I checked — it matches") the
primary button. Dismissing the warning is available, plainly labelled, and
recorded as *unverified* rather than verified — because tapping through a dialog
is not the same as comparing a fingerprint, and the record should not pretend it
was.

### 4.4 Only authenticated peers are recorded

TOFU records a peer **after** they prove possession of the private key, never
from an advertisement. Recording unauthenticated claims would let anyone poison
the store by broadcasting, turning the identity-change warning into noise.

## 5. Room join codes

A room has two identifiers:

| | `RoomId` | `JoinCode` |
|---|---|---|
| Looks like | 128 random bits | `ANV-7FK2-P9W4` |
| For | the protocol | reading aloud |
| Learned | at `RoomAccept` | from the host, out of band |

They are generated independently. Deriving the room id from the code would cap
the room's identity at the code's 40 bits, and room ids appear in packet headers.

### 5.1 Discovery without a registry

Nothing records that a room exists. Joining works by derivation:

```
  host                              joiner
  ────                              ──────
  JoinCode ─┐                   ┌─  user types the same code
            ▼                   ▼
       discovery token   ==   discovery token
            │                   │
  advertises it locally ◄───────┘  "who is advertising this?"
```

### 5.2 A join code is not access control

Eight base32 characters is 40 bits. An attacker who observes the advertised
token can brute-force the code offline — hours of compute, not a research
project. Lengthening it would help and would also make it unreadable over the
phone, which is the point of having one.

So, plainly:

* a join code stops **casual** joining by someone who did not hear it;
* a room needing real access control uses **host approval**, where a human
  decides;
* cryptographic membership is enforced regardless — guessing a code gets an
  attacker as far as *asking to join*, and no further.

### 5.3 The alphabet is chosen for speech

Crockford base32: no `I`, `L`, `O` or `U`. Parsing additionally corrects `O`→`0`,
`I`/`L`→`1` and `U`→`V`, accepts lower case, and ignores hyphens and spaces.
Someone reading "zero" aloud and the listener typing the letter O should simply
work — telling them "no such room" because of that is an unhelpful thing to say
to someone who typed exactly what they heard.

## 6. What has no answer

**Losing the device loses the identity.** There is no recovery, because recovery
requires an authority to recover *from*. Peers who knew that device will see a
new identity for the same name and — correctly — raise the change warning.

This is a genuine cost of having no accounts, not an oversight, and the first-run
screen says so rather than letting people discover it when they change phones.
Mitigations worth considering later — an exportable encrypted identity backup,
or multi-device identities — are real work and change the trust model, so they
belong in a version where they can be designed properly rather than bolted on.

## 7. Local storage

Everything durable lives on the device:

```
  profile          display name, peer id, created at
  identity keys    private key in Keychain / Android Keystore
  known peers      keys, names, first and last seen, trust state
  chat history     local record of what was said
  preferences
```

The identity **must not sync or restore** — `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`
on iOS, device-bound on Android. A restored identity means two phones claiming to
be the same peer, which the protocol has no way to resolve.
