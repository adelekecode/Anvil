package dev.anvil

import android.content.Context
import android.util.Log

/**
 * The Android side of the platform boundary (§9, §90).
 *
 * This class and the adapters it owns are the *only* Android-specific code in
 * Anvil. They expose capabilities and push events; they make no protocol
 * decisions. Specifically, nothing here may:
 *
 *  - choose between LAN and Wi-Fi Aware,
 *  - retry a failed connection,
 *  - de-duplicate discovered peers,
 *  - buffer, reorder or conceal media.
 *
 * Every one of those is a decision the Rust core makes, once, for both
 * platforms. The moment one of them leaks into Kotlin, Android and iOS start
 * behaving differently in ways nobody notices until a cross-platform call
 * sounds wrong.
 *
 * ## Threading
 *
 * Every callback below arrives on an arbitrary Android thread. They are all
 * forwarded straight into the core's single inbox, which is where ordering is
 * established. Do not add locks here; the core is the serialisation point.
 *
 * ## Phase status
 *
 * Phase 0: structure and the native bridge. The adapters are stubs with the
 * relevant Android APIs and their gotchas documented — Phases 1 and 4 fill them
 * in.
 */
class AnvilPlatform(private val context: Context) {

    private val lan = LanAdapter(context) { event -> emit(event) }
    private val aware = WifiAwareAdapter(context) { event -> emit(event) }
    private val audio = AudioAdapter(context) { event -> emit(event) }
    private val lifecycle = LifecycleAdapter { event -> emit(event) }

    /** Native session pointer, set by the Flutter side after `anvil_init`. */
    @Volatile
    private var sessionPtr: Long = 0L

    fun attach(sessionPtr: Long) {
        this.sessionPtr = sessionPtr
        lifecycle.start()
    }

    fun detach() {
        lifecycle.stop()
        lan.stopDiscovery()
        aware.stopDiscovery()
        audio.stopCapture()
        audio.stopPlayback()
        sessionPtr = 0L
    }

    /**
     * What this device can actually do right now.
     *
     * Re-queried whenever the network state changes, because Wi-Fi Aware
     * availability is not static: it depends on hardware support, OS version,
     * Wi-Fi being on, and location services being enabled. A device that had
     * Aware a minute ago may not have it now, and that is an ordinary
     * condition to degrade from, not a failure.
     */
    fun capabilities(): Capabilities = Capabilities(
        lan = lan.isAvailable(),
        wifiAware = aware.isAvailable(),
        microphone = Permissions.hasMicrophone(context),
        nearbyDevices = Permissions.hasNearbyDevices(context),
        secureKeyStorage = KeyStore.hasHardwareBackedStorage(context),
    )

    // --- calls from the core ------------------------------------------------

    fun startLanDiscovery() = lan.startDiscovery()
    fun stopLanDiscovery() = lan.stopDiscovery()
    fun startAwareDiscovery() = aware.startDiscovery()
    fun stopAwareDiscovery() = aware.stopDiscovery()

    fun advertise(payload: ByteArray) {
        lan.advertise(payload)
        aware.advertise(payload)
    }

    fun stopAdvertising() {
        lan.stopAdvertising()
        aware.stopAdvertising()
    }

    fun connect(pathId: Long, kind: String, address: String) {
        when (kind) {
            "lan" -> lan.connect(pathId, address)
            "wifi-aware" -> aware.connect(pathId, address)
            else -> Log.w(TAG, "unknown path kind $kind")
        }
    }

    fun startCapture(sampleRateHz: Int, channels: Int, frameMillis: Int) =
        audio.startCapture(sampleRateHz, channels, frameMillis)

    fun stopCapture() = audio.stopCapture()

    fun play(samples: ShortArray) = audio.play(samples)

    // --- events to the core -------------------------------------------------

    private fun emit(event: PlatformEvent) {
        val ptr = sessionPtr
        if (ptr == 0L) return
        nativeSubmitEvent(ptr, event.toJson())
    }

    /**
     * Hands a platform event to the Rust core.
     *
     * PHASE1: implemented in `crates/anvil-ffi` as a JNI entry point that
     * forwards into the same command queue the Flutter side uses, so platform
     * events and host commands are serialised in one place.
     */
    private external fun nativeSubmitEvent(sessionPtr: Long, json: String)

    companion object {
        private const val TAG = "Anvil"

        init {
            System.loadLibrary("anvil_ffi")
        }
    }
}

/** Mirrors `anvil_core::platform::Capabilities`. */
data class Capabilities(
    val lan: Boolean,
    val wifiAware: Boolean,
    val microphone: Boolean,
    val nearbyDevices: Boolean,
    val secureKeyStorage: Boolean,
)
