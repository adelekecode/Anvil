package dev.anvil

import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.aware.WifiAwareManager
import android.os.Build

/**
 * Wi-Fi Aware (NAN) on Android (§104).
 *
 * ## The shape of the work
 *
 * 1. `WifiAwareManager.attach()` → a `WifiAwareSession`. Attaching costs power
 *    and holds the radio in a discovery duty cycle; detach when not in a room.
 * 2. `publish()` a `PublishConfig` with the Anvil service name and our
 *    advertisement bytes as service-specific info; `subscribe()` with a
 *    matching `SubscribeConfig`.
 * 3. On `onServiceDiscovered`, hand the peer handle and service info to the
 *    core as a sighting. **Do not correlate peers here** — correlation is
 *    cryptographic and happens in Rust (§65).
 * 4. To connect, request a network with a
 *    `WifiAwareNetworkSpecifier` built from the `PeerHandle`, then use the
 *    resulting `Network` to bind a socket.
 *
 * ## Things that will cost time, listed in advance
 *
 * **Availability is conditional.** `WifiAwareManager.isAvailable` can be false
 * even on supporting hardware — Wi-Fi off, location services off, or the
 * framework having torn Aware down for its own reasons. It also *changes at
 * runtime*, broadcast via `ACTION_WIFI_AWARE_STATE_CHANGED`. Treat unavailable
 * as normal and degrade to LAN.
 *
 * **Permissions are awkward and version-dependent.** Aware needs
 * `ACCESS_FINE_LOCATION` on older releases and `NEARBY_WIFI_DEVICES` (with
 * `usesPermissionFlags="neverForLocation"`) from API 33. Getting this wrong
 * fails silently — discovery simply returns nothing, with no error.
 *
 * **Service-specific info is tiny.** On the order of a couple of hundred bytes,
 * shared with the rest of the advertisement. This is why the Anvil
 * advertisement carries an 8-byte fingerprint rather than a full identity key.
 *
 * **Addressing is IPv6 link-local, scoped to the Aware interface.** The address
 * is meaningless without its scope id, and it changes between sessions. Pass it
 * back to the core as an opaque string; the core never parses it.
 *
 * **Sockets must be bound to the Aware `Network`.** Using the default route
 * sends packets nowhere, or over cellular. `network.bindSocket()` or
 * `network.socketFactory` — not the process default, which would break the LAN
 * path at the same time.
 *
 * PHASE4.
 */
class WifiAwareAdapter(
    private val context: Context,
    private val emit: (PlatformEvent) -> Unit,
) {

    private val manager: WifiAwareManager? by lazy {
        if (!supportsAware()) null
        else context.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
    }

    /** Whether Aware is usable *right now*, not merely supported. */
    fun isAvailable(): Boolean = manager?.isAvailable == true

    private fun supportsAware(): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI_AWARE)

    fun startDiscovery() {
        // PHASE4: attach(), then publish() + subscribe() with the Anvil service
        // name. Emit PeerAdvertised on onServiceDiscovered.
        TODO("Phase 4: Wi-Fi Aware attach, publish and subscribe")
    }

    fun stopDiscovery() {
        // PHASE4: close discovery sessions; detach if not in a room.
    }

    fun advertise(payload: ByteArray) {
        // PHASE4: republish with updated service-specific info. Payload must be
        // under the platform limit — the core keeps it small, but check here
        // rather than letting the framework truncate silently.
    }

    fun stopAdvertising() {
        // PHASE4: close the publish session, keep subscribing.
    }

    fun connect(pathId: Long, address: String) {
        // PHASE4: build a WifiAwareNetworkSpecifier from the PeerHandle this
        // address encodes, requestNetwork, and emit PathEstablished with the
        // negotiated datagram size — or PathLost on failure. Do not retry here;
        // the core decides whether to try again.
        TODO("Phase 4: Wi-Fi Aware data path")
    }
}
