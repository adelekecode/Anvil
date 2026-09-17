package dev.anvil

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager

/**
 * App lifecycle and device status.
 *
 * The core needs to know about backgrounding because Android restricts what a
 * backgrounded app may do with radios and the microphone — and because a
 * backgrounded device is a poor relay candidate even when its network looks
 * excellent.
 *
 * Battery and charging state feed relay election (§37): a plugged-in phone is
 * the right device to carry a room, and one at 8% is not.
 *
 * PHASE1.
 */
class LifecycleAdapter(
    private val context: Context,
    private val emit: (PlatformEvent) -> Unit,
) {

    private var foreground = false
    private var batteryReceiver: BroadcastReceiver? = null

    fun start() {
        if (batteryReceiver == null) {
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) {
                    emitStatus(intent)
                }
            }
            batteryReceiver = receiver
            context.registerReceiver(receiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        }
        foreground()
    }

    fun stop() {
        batteryReceiver?.let { receiver ->
            context.unregisterReceiver(receiver)
            batteryReceiver = null
        }
        background()
    }

    fun foreground() {
        if (!foreground) {
            foreground = true
            emit(PlatformEvent.LifecycleChanged(true))
        }
    }

    fun background() {
        if (foreground) {
            foreground = false
            emit(PlatformEvent.LifecycleChanged(false))
        }
    }

    private fun emitStatus(intent: Intent) {
        val level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
        val percentage = if (level >= 0 && scale > 0) (level * 100 / scale).coerceIn(0, 100) else null
        val status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
        val charging = status == BatteryManager.BATTERY_STATUS_CHARGING ||
            status == BatteryManager.BATTERY_STATUS_FULL
        emit(PlatformEvent.DeviceStatus(percentage, charging, false))
    }
}
