package dev.anvil

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Base64
import android.util.Log
import java.util.UUID

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

    private val wifi: WifiManager by lazy {
        context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    }

    private var discoveryListener: NsdManager.DiscoveryListener? = null
    private var registrationListener: NsdManager.RegistrationListener? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private val visibleHandles = mutableSetOf<String>()
    private val serviceName = "Anvil-${UUID.randomUUID().toString().take(8)}"
    @Volatile private var advertisedPayload: ByteArray? = null

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
        if (discoveryListener != null) return

        multicastLock = wifi.createMulticastLock("anvil-nsd").apply {
            setReferenceCounted(false)
            acquire()
        }

        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) = Unit

            override fun onServiceFound(service: NsdServiceInfo) {
                if (!service.serviceType.startsWith(SERVICE_TYPE.removeSuffix("."))) return
                if (service.serviceName == serviceName) return
                resolve(service)
            }

            override fun onServiceLost(service: NsdServiceInfo) {
                synchronized(visibleHandles) { visibleHandles.remove(service.serviceName) }
                emit(PlatformEvent.PeerAdvertisementLost("lan", service.serviceName))
            }

            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "NSD discovery failed to start: $errorCode")
                discoveryListener = null
                releaseMulticastLock()
                emit(PlatformEvent.NetworkChanged("lan", false))
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "NSD discovery failed to stop: $errorCode")
            }
        }
        discoveryListener = listener
        try {
            nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (error: RuntimeException) {
            discoveryListener = null
            releaseMulticastLock()
            throw error
        }
    }

    fun stopDiscovery() {
        val listener = discoveryListener ?: return
        discoveryListener = null
        try {
            nsd.stopServiceDiscovery(listener)
        } catch (_: IllegalArgumentException) {
            // The framework may already have removed a failed listener.
        }
        val handles = synchronized(visibleHandles) {
            visibleHandles.toList().also { visibleHandles.clear() }
        }
        handles.forEach { emit(PlatformEvent.PeerAdvertisementLost("lan", it)) }
        releaseMulticastLock()
    }

    fun advertise(payload: ByteArray) {
        require(payload.size <= MAX_TXT_VALUE_BYTES) {
            "Anvil advertisement is ${payload.size} bytes; NSD TXT limit is $MAX_TXT_VALUE_BYTES"
        }
        if (advertisedPayload?.contentEquals(payload) == true && registrationListener != null) return
        stopAdvertising()
        advertisedPayload = payload.copyOf()

        val info = NsdServiceInfo().apply {
            serviceName = this@LanAdapter.serviceName
            serviceType = SERVICE_TYPE
            port = DEFAULT_PORT
            setAttribute(TXT_KEY, Base64.encodeToString(payload, Base64.NO_WRAP))
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(service: NsdServiceInfo) {
                Log.d(TAG, "Advertising ${service.serviceName} on port ${service.port}")
            }

            override fun onRegistrationFailed(service: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "NSD registration failed: $errorCode")
                if (registrationListener === this) registrationListener = null
            }

            override fun onServiceUnregistered(service: NsdServiceInfo) = Unit

            override fun onUnregistrationFailed(service: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "NSD unregistration failed: $errorCode")
            }
        }
        registrationListener = listener
        nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    fun stopAdvertising() {
        val listener = registrationListener ?: return
        registrationListener = null
        try {
            nsd.unregisterService(listener)
        } catch (_: IllegalArgumentException) {
            // Already removed after a framework registration failure.
        }
    }

    fun connect(pathId: Long, address: String) {
        // PHASE1: open a QUIC connection over a socket bound to the Wi-Fi
        // Network, then emit PathEstablished or PathLost carrying pathId.
        emit(PlatformEvent.PathLost(pathId, "LAN transport is not implemented"))
    }

    fun close(pathId: Long) = Unit

    fun sendDatagram(pathId: Long, data: ByteArray): Boolean = false

    fun sendReliable(pathId: Long, data: ByteArray): Boolean = false

    fun listen(): String = "0.0.0.0:$DEFAULT_PORT"

    @Suppress("DEPRECATION")
    private fun resolve(service: NsdServiceInfo) {
        nsd.resolveService(
            service,
            object : NsdManager.ResolveListener {
                override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                    Log.d(TAG, "Could not resolve ${serviceInfo.serviceName}: $errorCode")
                }

                override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                    val encodedPayload = serviceInfo.attributes[TXT_KEY] ?: return
                    val payload = try {
                        Base64.decode(encodedPayload.toString(Charsets.UTF_8), Base64.NO_WRAP)
                    } catch (_: IllegalArgumentException) {
                        return
                    }
                    val host = serviceInfo.host?.hostAddress ?: return
                    val socketAddress = if (host.contains(':')) {
                        "[$host]:${serviceInfo.port}"
                    } else {
                        "$host:${serviceInfo.port}"
                    }
                    synchronized(visibleHandles) { visibleHandles.add(serviceInfo.serviceName) }
                    emit(
                        PlatformEvent.PeerAdvertised(
                            kind = "lan",
                            handle = serviceInfo.serviceName,
                            address = socketAddress,
                            payload = payload,
                        ),
                    )
                }
            },
        )
    }

    private fun releaseMulticastLock() {
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
    }

    private companion object {
        const val TAG = "AnvilLan"
        const val SERVICE_TYPE = "_anvil._udp."
        const val TXT_KEY = "anvil"
        const val DEFAULT_PORT = 47_820
        const val MAX_TXT_VALUE_BYTES = 255
    }
}
