package dev.anvil

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat

/**
 * Permission checks.
 *
 * The manifest needs, at minimum:
 *
 * ```xml
 * <uses-permission android:name="android.permission.RECORD_AUDIO" />
 * <uses-permission android:name="android.permission.INTERNET" />
 * <uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
 * <uses-permission android:name="android.permission.CHANGE_WIFI_STATE" />
 * <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
 * <uses-permission android:name="android.permission.CHANGE_NETWORK_STATE" />
 *
 * <!-- Wi-Fi Aware and local discovery. NEARBY_WIFI_DEVICES from API 33; the
 *      neverForLocation flag matters, or Play review will ask why a voice app
 *      wants location. -->
 * <uses-permission
 *     android:name="android.permission.NEARBY_WIFI_DEVICES"
 *     android:usesPermissionFlags="neverForLocation"
 *     tools:targetApi="33" />
 * <uses-permission
 *     android:name="android.permission.ACCESS_FINE_LOCATION"
 *     android:maxSdkVersion="32" />
 *
 * <!-- Mandatory from Android 14 for microphone access outside the foreground. -->
 * <uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
 * <uses-permission android:name="android.permission.FOREGROUND_SERVICE_MICROPHONE" />
 *
 * <uses-feature android:name="android.hardware.wifi.aware" android:required="false" />
 * ```
 *
 * `android:required="false"` on the Aware feature is deliberate: requiring it
 * would hide Anvil from every device without Aware hardware, which is a large
 * fraction of the market and all of which can run the LAN path perfectly well.
 */
object Permissions {

    fun hasMicrophone(context: Context): Boolean =
        granted(context, Manifest.permission.RECORD_AUDIO)

    fun hasNearbyDevices(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            granted(context, Manifest.permission.NEARBY_WIFI_DEVICES)
        } else {
            granted(context, Manifest.permission.ACCESS_FINE_LOCATION)
        }

    private fun granted(context: Context, permission: String): Boolean =
        ContextCompat.checkSelfPermission(context, permission) ==
            PackageManager.PERMISSION_GRANTED
}
