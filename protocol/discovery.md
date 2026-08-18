# Discovery

How a device finds the people near it, before any trust exists.

## 1. Two mechanisms, one result

```
  LAN discovery (NSD / Bonjour)  ──┐
                                   ├──►  PeerTable  ──►  one row per person
  Wi-Fi Aware discovery ───────────┘
```

Discovery does **not** use the Anvil packet protocol. It rides the platform's own
mechanisms, because those are what work with no router, no DNS and no prior
contact. Both advertise the same service name, `_anvil._udp`, and carry the same
opaque payload.

## 2. The advertisement

Everything Anvil broadcasts to anyone listening. Total budget is small: Wi-Fi
Aware service-specific info is capped around 128 bytes, shared with the rest of
the advertisement, and Bonjour TXT records want to stay small too.

```
 offset  size  field
 0       1     version
 1       1     flags        bit 0 = hosting a joinable room
 2       8     fingerprint  truncated hash of the identity public key
 10      4     room hint    present only if bit 0 set; truncated RoomId
 next    1     name length  ≤ 48
 next    n     display name UTF-8
```

### 2.1 Why a fingerprint and not a key

A full 32-byte identity key plus a room id plus a name does not comfortably fit,
and padding it out would slow discovery on the transport where discovery is
already slowest. Eight bytes gives roughly a 1-in-2³² chance of accidental
collision among the handful of devices in radio range — ample for correlating
sightings, useless for security, which is the correct division of labour.

### 2.2 Nothing here is true

Every field is attacker-controlled. Anyone within range can advertise anyone's
fingerprint, any name, and any room hint. This data is a routing hint and a UI
convenience.

**The UI must show unconfirmed peers as unconfirmed.** Identity becomes real only
at the handshake, when the peer proves possession of the private key.

## 3. De-duplication

The problem: Alice's phone is on the same Wi-Fi *and* in Aware range. Both
mechanisms find her. Without correlation, the UI shows "Alice" twice, the user
does not know which to tap, and the transport layer treats her as two peers with
one path each — which quietly destroys the entire failover story, because
neither "peer" ever has a standby path.

Correlation is two-stage:

| Stage | Key | Trustworthy? | Good for |
|---|---|---|---|
| Provisional | advertised fingerprint | **no** | merging rows in a list |
| Confirmed | proven identity key | yes | everything else |

A peer that is provisionally correlated may be an impostor. `DiscoveredPeer` has
a `confirmed` flag so nothing downstream can forget that.

### 3.1 Losing one transport is not losing a peer

Removing a peer's LAN endpoint while their Aware endpoint still works must not
emit "peer lost". Doing so makes the UI flicker every time a router hiccups.

## 4. Liveness

Peers expire after **30 seconds** without a sighting, refreshed every **5
seconds**.

The TTL sweep is the primary departure mechanism, not a backstop. Both platforms
drop "service went away" callbacks — a phone that walks out of range usually
just stops advertising, with no event at all. The TTL is generous because Aware
discovery is duty-cycled and a peer can genuinely go quiet for several seconds
while sitting on the table.

## 5. Platform notes

### Android

- NSD resolution is serialised on older releases; concurrent resolves fail with
  `FAILURE_ALREADY_ACTIVE`. Queue them.
- `NEARBY_WIFI_DEVICES` from API 33 (with `neverForLocation`),
  `ACCESS_FINE_LOCATION` below it. Wrong permission = silent empty results.
- mDNS reception may need a `MulticastLock` on some devices.

### iOS

- `NSLocalNetworkUsageDescription` and `NSBonjourServices` listing `_anvil._udp`
  are both required. Missing either = silent empty results.
- **There is no API to query whether local network permission was granted.** The
  only signal is that browsing finds nothing — indistinguishable from an empty
  room. The join UX has to account for that ambiguity.

## 6. Open questions

- **Should the advertisement carry a room hint at all?** It tells passers-by that
  a room exists here. The alternative is join-by-code only, which is worse UX and
  better privacy. Currently: hint included, revisit if it matters.
- **Rotating fingerprints.** A static fingerprint lets a passive observer track a
  device across locations over time. Rotating it per session would break
  "recognise a peer met before", which is a real usability property. Not solved
  for v0.1; worth naming rather than ignoring.
