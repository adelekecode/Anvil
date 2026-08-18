package dev.anvil

import android.content.Context
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.os.Process
import java.util.concurrent.atomic.AtomicBoolean

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

    private val capturing = AtomicBoolean(false)
    @Volatile private var record: AudioRecord? = null
    @Volatile private var track: AudioTrack? = null
    private var captureThread: Thread? = null

    fun startCapture(sampleRateHz: Int, channels: Int, frameMillis: Int) {
        if (capturing.get()) return
        if (!Permissions.hasMicrophone(context)) {
            emit(PlatformEvent.PermissionChanged("microphone", false))
            return
        }
        val channelConfig = if (channels == 1) AudioFormat.CHANNEL_IN_MONO
        else AudioFormat.CHANNEL_IN_STEREO
        val frameSamples = sampleRateHz * frameMillis / 1000 * channels
        val minimum = AudioRecord.getMinBufferSize(
            sampleRateHz,
            channelConfig,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        val audioRecord = AudioRecord(
            MediaRecorder.AudioSource.VOICE_COMMUNICATION,
            sampleRateHz,
            channelConfig,
            AudioFormat.ENCODING_PCM_16BIT,
            maxOf(minimum, frameSamples * 8),
        )
        check(audioRecord.state == AudioRecord.STATE_INITIALIZED) { "AudioRecord initialization failed" }
        record = audioRecord
        capturing.set(true)
        audioRecord.startRecording()
        captureThread = Thread({
            Process.setThreadPriority(Process.THREAD_PRIORITY_AUDIO)
            var timestamp = 0L
            val samples = ShortArray(frameSamples)
            while (capturing.get()) {
                var offset = 0
                while (offset < samples.size && capturing.get()) {
                    val read = audioRecord.read(samples, offset, samples.size - offset)
                    if (read <= 0) break
                    offset += read
                }
                if (offset == samples.size) {
                    emit(PlatformEvent.AudioCaptured(samples.copyOf(), sampleRateHz, channels, timestamp))
                    timestamp = (timestamp + frameSamples / channels).and(0xFFFF_FFFFL)
                }
            }
        }, "anvil-audio-capture").also { it.start() }
    }

    fun stopCapture() {
        capturing.set(false)
        record?.let {
            try { it.stop() } catch (_: IllegalStateException) { }
            it.release()
        }
        record = null
        captureThread?.interrupt()
        captureThread = null
    }

    fun startPlayback(sampleRateHz: Int, channels: Int) {
        if (track != null) return
        val channelConfig = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO
        else AudioFormat.CHANNEL_OUT_STEREO
        val minimum = AudioTrack.getMinBufferSize(
            sampleRateHz,
            channelConfig,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        val audioTrack = AudioTrack.Builder()
            .setAudioAttributes(
                android.media.AudioAttributes.Builder()
                    .setUsage(android.media.AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(android.media.AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(sampleRateHz)
                    .setChannelMask(channelConfig)
                    .build(),
            )
            .setBufferSizeInBytes(maxOf(minimum, sampleRateHz / 10 * channels * 2))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .build()
        check(audioTrack.state == AudioTrack.STATE_INITIALIZED) { "AudioTrack initialization failed" }
        track = audioTrack
        audioTrack.play()
    }

    fun stopPlayback() {
        track?.let {
            try { it.stop() } catch (_: IllegalStateException) { }
            it.release()
        }
        track = null
    }

    fun play(samples: ShortArray) {
        track?.write(samples, 0, samples.size, AudioTrack.WRITE_NON_BLOCKING)
    }
}
