package dev.anvil

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
class LifecycleAdapter(private val emit: (PlatformEvent) -> Unit) {

    fun start() {
        // PHASE1: register a ProcessLifecycleOwner observer for foreground and
        // background, plus a BroadcastReceiver for ACTION_BATTERY_CHANGED and
        // thermal status via PowerManager.addThermalStatusListener.
    }

    fun stop() {
        // PHASE1: unregister everything.
    }
}
