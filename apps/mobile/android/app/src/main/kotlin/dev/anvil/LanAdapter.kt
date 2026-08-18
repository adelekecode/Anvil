package dev.anvil

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.nsd.NsdManager

/**
 * LAN discovery and connectivity on Android (§63).
 *
 * Uses NSD (`NsdManager`) to publish and browse `_anvil._udp`, with the Anvil
 * advertisement in the TXT record.
 *
 * ## The trap
 *
 * **A router is not the internet, and Android does not agree.** When the Wi-Fi
 * network has no internet access, Android marks it unvalidated and may route
 * the process's default traffic over cellular instead. Anvil's normal operating
 * condition is exactly that network, so every socket must be explicitly bound
 * to the Wi-Fi `Network`:
 *
 * ```kotlin
 * val request = NetworkRequest.Builder()
 *     .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
 *     // deliberately NOT NET_CAPABILITY_VALIDATED or NET_CAPABILITY_INTERNET
 *     .build()
 * connectivity.requestNetwork(request, callback)
 * // then network.bindSocket(socket) — never the process default, which would
 * // break the Wi-Fi Aware path at the same time.
 * ```
 *
 * Getting this wrong produces the most confusing possible symptom: discovery
 * works (mDNS is link-local) but every connection times out.
 *
 * ## Other things worth knowing
 *
 * - **NSD resolution is serialised** on older Android versions; resolving
 *   several peers concurrently fails with `FAILURE_ALREADY_ACTIVE`. Queue them.
 * - **Client isolation** is common on guest and enterprise Wi-Fi: peers can
 *   reach the gateway but not each other. Discovery succeeds, connections fail.
 *   The core handles this correctly already — the LAN path never reaches Ready,
 *   so Aware wins — but it deserves a distinct diagnostic rather than a generic
 *   timeout.
 * - **Multicast may need a `WifiManager.MulticastLock`** on some devices for
 *   mDNS to be received at all.
 *
 * PHASE1.
 */
class LanAdapter(
    private val context: Context,
    private val emit: (PlatformEvent) -> Unit,
) {

    private val nsd: NsdManager by lazy {
        context.getSystemService(Context.NSD_SERVICE) as NsdManager
    }

    private val connectivity: ConnectivityManager by lazy {
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    }

    /**
     * Whether a Wi-Fi network is attached.
     *
     * Note what is *not* checked: `NET_CAPABILITY_VALIDATED` and
     * `NET_CAPABILITY_INTERNET`. A router with the WAN cable pulled is a
     * perfectly good Anvil network.
     */
    fun isAvailable(): Boolean {
        val network = connectivity.activeNetwork ?: return false
        val caps = connectivity.getNetworkCapabilities(network) ?: return false
        return caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
    }

    fun startDiscovery() {
        // PHASE1: discoverServices("_anvil._udp", PROTOCOL_DNS_SD, listener).
        // On onServiceFound, resolve (serialised) and emit PeerAdvertised with
        // the TXT payload and host:port.
        TODO("Phase 1: NSD browse")
    }

    fun stopDiscovery() {
        // PHASE1: stopServiceDiscovery.
    }

    fun advertise(payload: ByteArray) {
        // PHASE1: registerService with the payload as a TXT attribute. Update by
        // unregistering and re-registering — NSD has no in-place update.
    }

    fun stopAdvertising() {
        // PHASE1: unregisterService.
    }

    fun connect(pathId: Long, address: String) {
        // PHASE1: open a QUIC connection over a socket bound to the Wi-Fi
        // Network, then emit PathEstablished or PathLost carrying pathId.
        TODO("Phase 1: LAN QUIC connect")
    }
}
