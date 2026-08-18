package dev.anvil

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore as JavaKeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

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
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.M &&
            (context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE) ||
                context.packageManager.hasSystemFeature("android.hardware.hardware_keystore"))

    fun loadIdentity(context: Context): ByteArray? {
        val encoded = preferences(context).getString(PREF_VALUE, null) ?: return null
        val stored = Base64.decode(encoded, Base64.NO_WRAP)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return stored
        require(stored.size > IV_BYTES) { "stored Anvil identity is truncated" }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(
            Cipher.DECRYPT_MODE,
            getOrCreateWrappingKey(),
            GCMParameterSpec(TAG_BITS, stored, 0, IV_BYTES),
        )
        return cipher.doFinal(stored, IV_BYTES, stored.size - IV_BYTES)
    }

    fun storeIdentity(context: Context, bytes: ByteArray) {
        val stored = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.ENCRYPT_MODE, getOrCreateWrappingKey())
            cipher.iv + cipher.doFinal(bytes)
        } else {
            // Android 5 has no symmetric-key Android Keystore API. App-private
            // storage is the honest fallback and capabilities reports no
            // hardware-backed guarantee on these releases.
            bytes.copyOf()
        }
        preferences(context).edit()
            .putString(PREF_VALUE, Base64.encodeToString(stored, Base64.NO_WRAP))
            .apply()
    }

    fun clearIdentity(context: Context) {
        preferences(context).edit().remove(PREF_VALUE).apply()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            javaKeyStore().deleteEntry(KEY_ALIAS)
        }
    }

    private fun preferences(context: Context) =
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    private fun javaKeyStore(): JavaKeyStore = JavaKeyStore.getInstance("AndroidKeyStore").apply {
        load(null)
    }

    private fun getOrCreateWrappingKey(): SecretKey {
        val existing = javaKeyStore().getKey(KEY_ALIAS, null) as? SecretKey
        if (existing != null) return existing

        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }

    private const val KEY_ALIAS = "dev.anvil.identity.wrap.v1"
    private const val PREFERENCES = "dev.anvil.identity"
    private const val PREF_VALUE = "ciphertext"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val IV_BYTES = 12
    private const val TAG_BITS = 128
}
