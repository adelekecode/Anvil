package dev.anvil

import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.ActivityCompat

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
        val result = nativeAttach(sessionPtr)
        check(result == 0) { "Rust rejected Android platform attachment: $result" }
        this.sessionPtr = sessionPtr
        lifecycle.start()
    }

    fun detach() {
        val ptr = sessionPtr
        lifecycle.stop()
        lan.stopDiscovery()
        lan.stopAdvertising()
        aware.stopDiscovery()
        audio.stopCapture()
        audio.stopPlayback()
        sessionPtr = 0L
        if (ptr != 0L) nativeDetach(ptr)
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

    /** Compact, allocation-free form consumed by the JNI adapter. */
    fun capabilitiesMask(): Int {
        val value = capabilities()
        return (if (value.lan) 1 else 0) or
            (if (value.wifiAware) 2 else 0) or
            (if (value.microphone) 4 else 0) or
            (if (value.nearbyDevices) 8 else 0) or
            (if (value.secureKeyStorage) 16 else 0)
    }

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

    fun startPlayback(sampleRateHz: Int, channels: Int) =
        audio.startPlayback(sampleRateHz, channels)

    fun stopPlayback() = audio.stopPlayback()

    fun play(samples: ShortArray) = audio.play(samples)

    fun close(pathId: Long) {
        lan.close(pathId)
        aware.close(pathId)
    }

    fun sendDatagram(pathId: Long, data: ByteArray) {
        if (!lan.sendDatagram(pathId, data)) aware.sendDatagram(pathId, data)
    }

    fun sendReliable(pathId: Long, data: ByteArray) {
        if (!lan.sendReliable(pathId, data)) aware.sendReliable(pathId, data)
    }

    fun listen(kind: String): String = when (kind) {
        "lan" -> lan.listen()
        "wifi-aware" -> aware.listen()
        else -> error("unknown path kind $kind")
    }

    fun loadIdentity(): ByteArray? = KeyStore.loadIdentity(context)
    fun storeIdentity(bytes: ByteArray) = KeyStore.storeIdentity(context, bytes)
    fun clearIdentity() = KeyStore.clearIdentity(context)

    fun requestPermission(capability: String) {
        // MUST be android.app.Activity, not androidx ComponentActivity.
        //
        // FlutterActivity extends android.app.Activity directly — it is *not* a
        // ComponentActivity. Casting to ComponentActivity therefore always
        // failed here, so this method silently reported "denied" and never
        // showed a dialog: on Android the app could never ask for the
        // microphone or nearby-devices permission at all. ActivityCompat only
        // needs an Activity anyway.
        val activity = context as? Activity
        if (activity == null) {
            Log.w(TAG, "cannot request $capability: context is not an Activity")
            emit(PlatformEvent.PermissionChanged(capability, false))
            return
        }
        val permission = when (capability) {
            "microphone" -> android.Manifest.permission.RECORD_AUDIO
            "nearby_devices" -> if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                android.Manifest.permission.NEARBY_WIFI_DEVICES
            } else {
                android.Manifest.permission.ACCESS_FINE_LOCATION
            }
            else -> {
                emit(PlatformEvent.PermissionChanged(capability, false))
                return
            }
        }
        ActivityCompat.requestPermissions(activity, arrayOf(permission), permissionRequestCode(capability))
    }

    fun onRequestPermissionsResult(
        requestCode: Int,
        grantResults: IntArray,
    ) {
        val capability = when (requestCode) {
            REQUEST_MICROPHONE -> "microphone"
            REQUEST_NEARBY -> "nearby_devices"
            else -> return
        }
        emit(
            PlatformEvent.PermissionChanged(
                capability,
                grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED,
            ),
        )
    }

    private fun permissionRequestCode(capability: String): Int =
        if (capability == "microphone") REQUEST_MICROPHONE else REQUEST_NEARBY

    // --- events to the core -------------------------------------------------

    private fun emit(event: PlatformEvent) {
        val ptr = sessionPtr
        if (ptr == 0L) return
        val result = nativeSubmitEvent(ptr, event.toJson())
        if (result != 0) Log.w(TAG, "platform event rejected by core: $result")
    }

    /**
     * Hands a platform event to the Rust core.
     *
     * PHASE1: implemented in `crates/anvil-ffi` as a JNI entry point that
     * forwards into the same command queue the Flutter side uses, so platform
     * events and host commands are serialised in one place.
     */
    private external fun nativeSubmitEvent(sessionPtr: Long, json: String): Int
    private external fun nativeAttach(sessionPtr: Long): Int
    private external fun nativeDetach(sessionPtr: Long)

    companion object {
        private const val TAG = "Anvil"
        private const val REQUEST_MICROPHONE = 4101
        private const val REQUEST_NEARBY = 4102

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
