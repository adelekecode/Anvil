package dev.anvil

import org.json.JSONArray
import org.json.JSONObject

/**
 * Events pushed to the Rust core, normalised so that a Kotlin
 * `WifiAwareSession` callback and a Swift `NWBrowser` result arrive looking
 * identical (§9, §90).
 *
 * JSON rather than a generated binding, for the same reason as the Flutter
 * boundary: the volume is low, and a boundary a human can read in a log is
 * worth more here than the microseconds a binary encoding would save. Media
 * never crosses this boundary as JSON — audio frames go over a direct buffer
 * handoff.
 */
sealed class PlatformEvent {

    abstract fun toJson(): String

    data class PeerAdvertised(
        val kind: String,
        val handle: String,
        val address: String,
        val payload: ByteArray,
    ) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "peerAdvertised")
            put("kind", kind)
            put("handle", handle)
            put("address", address)
            put("payload", JSONArray(payload.map { it.toInt() and 0xFF }))
        }.toString()
    }

    data class PeerAdvertisementLost(val kind: String, val handle: String) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "peerAdvertisementLost")
            put("kind", kind)
            put("handle", handle)
        }.toString()
    }

    data class PathEstablished(val pathId: Long, val maxDatagramSize: Int) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "pathEstablished")
            put("pathId", pathId)
            put("maxDatagramSize", maxDatagramSize)
        }.toString()
    }

    data class PathLost(val pathId: Long, val reason: String) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "pathLost")
            put("pathId", pathId)
            put("reason", reason)
        }.toString()
    }

    data class NetworkChanged(val kind: String, val available: Boolean) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "networkChanged")
            put("kind", kind)
            put("available", available)
        }.toString()
    }

    data class PermissionChanged(val capability: String, val granted: Boolean) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "permissionChanged")
            put("capability", capability)
            put("granted", granted)
        }.toString()
    }

    data class DeviceStatus(
        val batteryPct: Int?,
        val charging: Boolean,
        val thermallyThrottled: Boolean,
    ) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "deviceStatus")
            put("batteryPct", batteryPct ?: JSONObject.NULL)
            put("charging", charging)
            put("thermallyThrottled", thermallyThrottled)
        }.toString()
    }

    data class LifecycleChanged(val foreground: Boolean) : PlatformEvent() {
        override fun toJson(): String = JSONObject().apply {
            put("type", "lifecycleChanged")
            put("foreground", foreground)
        }.toString()
    }
}
