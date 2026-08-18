# §95 — LAN room, no internet

## Setup

- One Wi-Fi router, **WAN cable physically unplugged**. Not just "internet down"
  — unplugged, so there is no ambiguity about what is being tested.
- 3–4 devices, all joined to that network, **cellular data off** on every one.
- Diagnostics view open on each.

## Steps

1. Launch Anvil on all devices.
2. Confirm each device discovers the others within a few seconds.
3. Device A creates a room. B, C, D join.
4. Each participant speaks in turn; every other participant confirms they hear it.
5. Hold the call for five minutes.
6. Mute and unmute on each device.
7. Leave, one at a time.

## Expected

- Discovery completes with no internet available at any point.
- Room creation and joining work.
- Audio is intelligible in every direction.
- Diagnostics shows all paths as `lan`.
- **No device makes an internet request.** Verify independently — router logs, or
  a packet capture on the router — rather than trusting the app.

## Failure signatures worth recognising

| Symptom | Likely cause |
|---|---|
| Discovery works, all connections time out | client isolation on the AP, or sockets not bound to the Wi-Fi network |
| Discovery finds nothing on Android | `NEARBY_WIFI_DEVICES` / `ACCESS_FINE_LOCATION` not granted |
| Discovery finds nothing on iOS | local network permission denied, or `NSBonjourServices` missing |
| Audio one-way | NAT or firewall on the AP; check both directions separately |
| Works with internet, fails without | something is treating an unvalidated network as unusable — the exact bug this test exists to catch |

## Record

Join time · mouth-to-ear latency · concealment rate over five minutes ·
path switches (should be zero) · battery drain per device.
