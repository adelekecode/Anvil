package dev.anvil

import android.content.Context
import android.media.AudioManager

/**
 * Microphone capture and speaker playback on Android.
 *
 * ## Choices that matter
 *
 * **`AudioRecord`/`AudioTrack`, not `MediaRecorder`.** Anvil needs raw PCM
 * frames on a fixed cadence; the higher-level APIs are built for files.
 *
 * **`VOICE_COMMUNICATION` audio source.** Enables the platform's echo
 * cancellation, noise suppression and gain control. Without it, a
 * speakerphone room feeds back immediately — and Anvil rooms are frequently
 * speakerphone rooms, since the people are in the same physical space.
 *
 * **Low-latency output.** Request `PERFORMANCE_MODE_LOW_LATENCY` and use the
 * device's native sample rate and buffer size
 * (`PROPERTY_OUTPUT_SAMPLE_RATE` / `PROPERTY_OUTPUT_FRAMES_PER_BUFFER`) where
 * they match the configured format; mismatches force a resampler into the path
 * and add delay.
 *
 * **A foreground service is mandatory.** From Android 14, microphone access
 * from the background requires a foreground service with
 * `FOREGROUND_SERVICE_TYPE_MICROPHONE`. Without it the mic silently returns
 * silence when the user switches apps — during a call, which is exactly when
 * they will.
 *
 * ## Not Anvil's transport
 *
 * A Bluetooth headset here is the *user's* audio route (§87). It has nothing to
 * do with which radio carries the room, and the two must not be conflated in
 * code or in the UI.
 *
 * PHASE1.
 */
class AudioAdapter(
    private val context: Context,
    private val emit: (PlatformEvent) -> Unit,
) {

    private val audioManager: AudioManager by lazy {
        context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    }

    fun startCapture(sampleRateHz: Int, channels: Int, frameMillis: Int) {
        // PHASE1: AudioRecord with MediaRecorder.AudioSource.VOICE_COMMUNICATION,
        // read on a dedicated thread at frameMillis cadence, emit AudioCaptured.
        TODO("Phase 1: AudioRecord capture")
    }

    fun stopCapture() {
        // PHASE1: stop and release.
    }

    fun startPlayback(sampleRateHz: Int, channels: Int) {
        // PHASE1: AudioTrack in streaming mode, low-latency performance mode.
        TODO("Phase 1: AudioTrack playback")
    }

    fun stopPlayback() {
        // PHASE1: stop and release.
    }

    fun play(samples: ShortArray) {
        // PHASE1: AudioTrack.write. Must not block past the frame duration —
        // overrunning it is an underrun the user hears.
    }
}
