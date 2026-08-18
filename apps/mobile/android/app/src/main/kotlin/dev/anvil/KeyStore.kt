package dev.anvil

import android.content.Context
import android.content.pm.PackageManager

/**
 * Identity key storage (§82).
 *
 * The device identity key should live in the Android Keystore, ideally
 * hardware-backed (StrongBox where present, TEE otherwise), and ideally be
 * *used* there rather than exported.
 *
 * Note the awkwardness worth planning for: the Keystore's Ed25519 support is
 * uneven across vendors and API levels. If a hardware-backed Ed25519 key cannot
 * be created, the fallback is a software key wrapped by a hardware-backed AES
 * key — still meaningfully better than a plain file — and
 * [hasHardwareBackedStorage] must then report false so the diagnostics view
 * tells the truth rather than implying a guarantee the device is not providing.
 *
 * PHASE2.
 */
object KeyStore {

    fun hasHardwareBackedStorage(context: Context): Boolean =
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE) ||
            context.packageManager.hasSystemFeature("android.hardware.hardware_keystore")

    fun loadIdentity(): ByteArray? {
        // PHASE2
        return null
    }

    fun storeIdentity(bytes: ByteArray) {
        // PHASE2
    }

    fun clearIdentity() {
        // PHASE2
    }
}
