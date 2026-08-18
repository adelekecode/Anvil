# Room lifecycle

## 1. What a room is

A set of authenticated participants sharing a key epoch. Not a connection, not a
relay, not a network — it survives all three changing.

```
RoomState
├── room_id           cryptographically random, depends on nothing
├── epoch             advances on every membership change
├── participants      BTreeMap, so every device iterates identically
├── relay             None during an election, and in two-person rooms
├── local_peer_id
└── is_host
```

`RoomId` is 16 random bytes. It must not encode the creator, the relay, an IP or
a device — a room whose id depends on its relay cannot survive relay failover.

## 2. Creation

```
User taps Create
      ↓
generate RoomId
      ↓
local crypto state, epoch 0
      ↓
advertise the room hint
      ↓
nearby peers can request entry
```

The creator hosts, which for v0.1 means they handle admission. Hosting is not
authority over the room's *existence* — see §6.

## 3. Admission — *partly unpinned*

No cloud, no accounts, no phone numbers. Admission has to work between devices
that have never met, with no shared infrastructure. The one trust anchor Anvil
actually has is that the participants are physically together.

| Policy | How | Trade-off |
|---|---|---|
| `HostApproval` | host taps accept | Hard to attack; needs the host looking at their phone, awkward when the host is the one talking |
| `JoinCode` | code or QR on the host's screen | Good for a group forming at once; anyone who can read the screen — including over a shoulder or in a photo — can join |
| `Open` | anyone nearby | Only for genuinely public rooms; must look visibly different in the UI |

Join codes are compared in constant time. A six-digit code is low-entropy enough
that a timing oracle on the comparison is a real shortcut.

**Unpinned:** whether the credential is the code itself or a proof derived from
it. Sending the code means the host learns nothing new (they chose it), but it
does travel over the session; deriving a proof is stronger and more work. Decide
before Phase 1.

## 4. Joining

```
 discover room
      ↓
 RoomJoin  ──────────►
      ↓
 identity authentication (both directions)
      ↓
 admission decision
      ↓
 RoomAccept ◄────────  room id, epoch, members, relay
      ↓
 epoch advances; every member distributes their key for the new epoch
      ↓
 transport path selected
      ↓
 participant is in the room
```

Note the ordering: **identity is authenticated before admission is decided.** A
host approving "Bob" must be approving a proven identity, not an advertised name
anyone could claim.

## 5. Membership changes

Every change advances the epoch and forces new key material. That is what makes
departure mean something.

```
 David joins  →  epoch 41 → 42, all members rekey
 David leaves →  epoch 42 → 43, all members rekey, David's keys dropped
```

**Epochs only move forward.** A membership message describing an epoch that is
not newer than the current one is discarded. This is not an optimisation: it is
what stops a replayed join message from resurrecting a departed member.

Consequence for cost: an n-member room performs O(n²) key deliveries per change.
See [encryption.md](encryption.md) §5.2 for where that stops being acceptable.

## 6. There is no authority

Room state is distributed. Every participant holds their own copy; they converge
through authenticated control messages. There is no database and no canonical
node — **including the host and the relay**.

The relay may help distribute topology, but the room must survive it being
replaced. The host handles admission, but the room must survive the host
leaving. Anything that would break if the host disconnected is a design error.

**Unpinned:** what happens to admission when the host leaves a room that is still
populated. Options: hand admission to the next member by `PeerId` order; close
the room to new joins; or elect an admitter the way a relay is elected. The first
is simplest and consistent with the tie-break rule used elsewhere.

## 7. Topology

Derived from membership and the relay, never stored:

| Members | Relay | Topology | Media goes |
|---|---|---|---|
| ≤1 | any | `Pending` | nowhere |
| 2 | ignored | `Direct` | peer to peer |
| 3+ | elected | `Relayed` | to the relay |
| 3+ | none | `Pending` | nowhere, briefly |

`Pending` is a *live room with no route* — during an election, or while every
path is down. It is distinct from "not in a room", and the UI should say
something reassuring rather than nothing.

## 8. Leaving

Voluntary: announce, stop capture and playback, tear down paths, drop key
material. Remaining members advance the epoch.

Involuntary — battery dead, walked away, app killed — looks identical to the
others after the peer timeout, except the departing device announced nothing. The
remaining members must reach the same state either way, which means departure
cannot depend on receiving a message.

## 9. States the UI shows

| State | Meaning |
|---|---|
| `Discovering` | looking for peers |
| `CreatingRoom` / `JoiningRoom` | transient |
| `Connected` | in a room, media flowing |
| `Reconnecting` | in a room, path changing — **the room still exists** |
| `RelayElection` | in a room, picking a new relay |
| `Leaving` | tearing down |

`Reconnecting` and `RelayElection` are the two that matter for user trust. Both
mean "your room is fine, the network is being reorganised", and saying so is far
better than unexplained silence.
