package dev.anvil

import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.DiscoverySession
import android.net.wifi.aware.DiscoverySessionCallback
import android.net.wifi.aware.PeerHandle
import android.net.wifi.aware.PublishConfig
import android.net.wifi.aware.PublishDiscoverySession
import android.net.wifi.aware.SubscribeConfig
import android.net.wifi.aware.SubscribeDiscoverySession
import android.net.wifi.aware.WifiAwareManager
import android.net.wifi.aware.WifiAwareSession
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.nio.ByteBuffer
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * Android Wi-Fi Aware discovery-session transport.
 *
 * Aware discovery sessions have a small, bidirectional message channel. That
 * is a better fit for Anvil than pretending that an Aware peer has a stable IP
 * address: the peer handle is only meaningful for this Aware session and the
 * framework can change the NDP address when the radio is re-created. Messages
 * are framed here into Anvil datagrams and fragmented control records, while
 * the Rust core still owns authentication, routing and media timing.
 *
 * Aware is optional. Every framework failure becomes a NetworkChanged or
 * PathLost event; no callback is allowed to escape into the JNI boundary.
 */
class WifiAwareAdapter(
    private val context: Context,
    private val emit: (PlatformEvent) -> Unit,
) {

    private val manager: WifiAwareManager? by lazy {
        if (!supportsAware()) null
        else context.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
    }
    private val handler = Handler(Looper.getMainLooper())
    private val sessionLock = Any()
    private val nextMessageId = AtomicInteger(1)
    private val linksByAddress = ConcurrentHashMap<String, LinkState>()
    private val addressByPeer = ConcurrentHashMap<PeerHandle, String>()
    private val linksByPath = ConcurrentHashMap<Long, LinkState>()

    @Volatile private var awareSession: WifiAwareSession? = null
    @Volatile private var attaching = false
    @Volatile private var advertisedPayload: ByteArray? = null
    private var publishSession: PublishDiscoverySession? = null
    private var subscribeSession: SubscribeDiscoverySession? = null

    private class LinkState(
        val address: String,
        val peer: PeerHandle,
        @Volatile var session: DiscoverySession,
    ) {
        val sendLock = Any()
        val nextReliableId = AtomicInteger(1)
        val receiveLock = Any()
        var nextInboundReliableId = 1
        val partial = HashMap<Int, PartialMessage>()
        val complete = java.util.TreeMap<Int, ByteArray>()
        val pending = HashMap<Int, PendingMessage>()
        @Volatile var pathId: Long? = null
    }

    private class PartialMessage(val total: Int) {
        val parts = arrayOfNulls<ByteArray>(total)
        var received = 0
        var bytes = 0
    }

    private class PendingMessage(val frames: List<ByteArray>, var attempts: Int = 1)

    /** Whether Aware is usable *right now*, not merely supported. */
    fun isAvailable(): Boolean = try {
        supportsAware() && hasPermission() && manager?.isAvailable == true
    } catch (_: RuntimeException) {
        false
    }

    private fun supportsAware(): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI_AWARE)

    private fun hasPermission(): Boolean = Permissions.hasNearbyDevices(context)

    fun startDiscovery() {
        if (!isAvailable()) {
            emit(PlatformEvent.NetworkChanged("wifi-aware", false))
            return
        }
        synchronized(sessionLock) {
            if (awareSession != null || attaching) return
            attaching = true
        }
        try {
            manager?.attach(object : AttachCallback() {
                override fun onAttached(session: WifiAwareSession) {
                    synchronized(sessionLock) {
                        attaching = false
                        awareSession = session
                    }
                    emit(PlatformEvent.NetworkChanged("wifi-aware", true))
                    openDiscoverySessions(session)
                }

                override fun onAttachFailed() {
                    synchronized(sessionLock) { attaching = false }
                    emit(PlatformEvent.NetworkChanged("wifi-aware", false))
                }

                override fun onAwareSessionTerminated() {
                    synchronized(sessionLock) {
                        attaching = false
                        awareSession = null
                        publishSession = null
                        subscribeSession = null
                    }
                    failAllPaths("Wi-Fi Aware session terminated")
                    emit(PlatformEvent.NetworkChanged("wifi-aware", false))
                }
            }, handler)
        } catch (error: RuntimeException) {
            synchronized(sessionLock) { attaching = false }
            Log.w(TAG, "Wi-Fi Aware attach failed", error)
            emit(PlatformEvent.NetworkChanged("wifi-aware", false))
        }
    }

    fun stopDiscovery() {
        val publish: PublishDiscoverySession?
        val subscribe: SubscribeDiscoverySession?
        val session: WifiAwareSession?
        synchronized(sessionLock) {
            publish = publishSession
            subscribe = subscribeSession
            session = awareSession
            publishSession = null
            subscribeSession = null
            awareSession = null
        }
        try { publish?.close() } catch (_: RuntimeException) { }
        try { subscribe?.close() } catch (_: RuntimeException) { }
        try { session?.close() } catch (_: RuntimeException) { }
        clearLinks("Wi-Fi Aware discovery stopped")
    }

    fun advertise(payload: ByteArray) {
        if (payload.size > MAX_ADVERTISEMENT_BYTES) {
            Log.w(TAG, "Aware advertisement is ${payload.size} bytes; limit is $MAX_ADVERTISEMENT_BYTES")
            return
        }
        advertisedPayload = payload.copyOf()
        val session = awareSession ?: return
        if (publishSession == null) openPublishSession(session)
        else try {
            publishSession?.updatePublish(publishConfig(payload))
        } catch (error: RuntimeException) {
            Log.w(TAG, "Could not update Wi-Fi Aware advertisement", error)
        }
    }

    fun stopAdvertising() {
        val publish = synchronized(sessionLock) {
            publishSession.also { publishSession = null }
        }
        try { publish?.close() } catch (_: RuntimeException) { }
    }

    fun connect(pathId: Long, address: String) {
        val link = linksByAddress[address]
        if (link == null) {
            emit(PlatformEvent.PathLost(pathId, "unknown Wi-Fi Aware peer $address"))
            return
        }
        linksByPath[pathId] = link
        link.pathId = pathId
        emit(PlatformEvent.PathEstablished(pathId, MAX_DATAGRAM_PAYLOAD))
    }

    fun close(pathId: Long) {
        val link = linksByPath.remove(pathId) ?: return
        if (link.pathId == pathId) link.pathId = null
    }

    fun sendDatagram(pathId: Long, data: ByteArray): Boolean {
        if (data.size > MAX_DATAGRAM_PAYLOAD) {
            Log.w(TAG, "Aware datagram is ${data.size} bytes; limit is $MAX_DATAGRAM_PAYLOAD")
            return false
        }
        val link = linksByPath[pathId] ?: return false
        return sendFrame(link, frame(FRAME_DATAGRAM, 0, 0, 1, data), pathId)
    }

    fun sendReliable(pathId: Long, data: ByteArray): Boolean {
        if (data.size > MAX_CONTROL_BYTES) return false
        val link = linksByPath[pathId] ?: return false
        val chunks = if (data.isEmpty()) listOf(ByteArray(0))
        else data.asList().chunked(MAX_FRAME_PAYLOAD).map { it.toByteArray() }
        val messageId = link.nextReliableId.getAndIncrement()
        val frames = chunks.mapIndexed { index, chunk ->
            frame(FRAME_RELIABLE, messageId, index, chunks.size, chunk)
        }
        synchronized(link.receiveLock) { link.pending[messageId] = PendingMessage(frames) }
        val sent = sendFrames(link, frames, pathId)
        if (sent) scheduleRetry(link, messageId, pathId)
        return sent
    }

    /** Aware discovery does not need an IP listener; peer handles are endpoints. */
    fun listen(): String = "aware://anvil"

    private fun openDiscoverySessions(session: WifiAwareSession) {
        openPublishSession(session)
        try {
            session.subscribe(
                SubscribeConfig.Builder().setServiceName(SERVICE_NAME).build(),
                AwareDiscoveryCallback(),
                handler,
            )
        } catch (error: RuntimeException) {
            Log.w(TAG, "Wi-Fi Aware subscribe failed", error)
            emit(PlatformEvent.NetworkChanged("wifi-aware", false))
        }
    }

    private fun openPublishSession(session: WifiAwareSession) {
        if (publishSession != null) return
        try {
            session.publish(
                publishConfig(advertisedPayload ?: ByteArray(0)),
                AwareDiscoveryCallback(),
                handler,
            )
        } catch (error: RuntimeException) {
            Log.w(TAG, "Wi-Fi Aware publish failed", error)
            emit(PlatformEvent.NetworkChanged("wifi-aware", false))
        }
    }

    private fun publishConfig(payload: ByteArray): PublishConfig =
        PublishConfig.Builder()
            .setServiceName(SERVICE_NAME)
            .setServiceSpecificInfo(payload)
            .build()

    private inner class AwareDiscoveryCallback : DiscoverySessionCallback() {
        private var session: DiscoverySession? = null

        override fun onPublishStarted(session: PublishDiscoverySession) {
            this.session = session
            synchronized(sessionLock) { publishSession = session }
        }

        override fun onSubscribeStarted(session: SubscribeDiscoverySession) {
            this.session = session
            synchronized(sessionLock) { subscribeSession = session }
        }

        override fun onServiceDiscovered(
            peerHandle: PeerHandle,
            serviceSpecificInfo: ByteArray,
            matchFilter: MutableList<ByteArray>,
        ) {
            val discovery = session ?: return
            val address = addressByPeer[peerHandle] ?: "aware-${UUID.randomUUID()}".also {
                addressByPeer[peerHandle] = it
            }
            val link = linksByAddress[address]
            if (link == null) linksByAddress[address] = LinkState(address, peerHandle, discovery)
            else link.session = discovery
            emit(
                PlatformEvent.PeerAdvertised(
                    kind = "wifi-aware",
                    handle = address,
                    address = address,
                    payload = serviceSpecificInfo.copyOf(),
                ),
            )
        }

        override fun onServiceLost(peerHandle: PeerHandle, reason: Int) {
            val address = addressByPeer.remove(peerHandle) ?: return
            val link = linksByAddress.remove(address) ?: return
            val path = link.pathId
            if (path != null) {
                linksByPath.remove(path)
                emit(PlatformEvent.PathLost(path, "Wi-Fi Aware peer lost ($reason)"))
            }
            emit(PlatformEvent.PeerAdvertisementLost("wifi-aware", address))
        }

        override fun onSessionConfigFailed() {
            emit(PlatformEvent.NetworkChanged("wifi-aware", false))
        }

        override fun onSessionTerminated() {
            emit(PlatformEvent.NetworkChanged("wifi-aware", false))
        }

        override fun onMessageReceived(peerHandle: PeerHandle, message: ByteArray) {
            val address = addressByPeer[peerHandle] ?: return
            val link = linksByAddress[address] ?: return
            receiveFrame(link, message)
        }
    }

    private fun receiveFrame(link: LinkState, bytes: ByteArray) {
        if (bytes.size < HEADER_BYTES) return
        val buffer = ByteBuffer.wrap(bytes)
        if (buffer.get() != MAGIC_0 || buffer.get() != MAGIC_1) return
        val type = buffer.get().toInt() and 0xff
        val id = buffer.int
        val index = buffer.short.toInt() and 0xffff
        val total = buffer.short.toInt() and 0xffff
        val payload = ByteArray(buffer.remaining()).also { buffer.get(it) }
        val path = link.pathId ?: return

        when (type) {
            FRAME_DATAGRAM -> if (total == 1 && index == 0) {
                emit(PlatformEvent.DatagramReceived(path, payload))
            }
            FRAME_ACK -> synchronized(link.receiveLock) { link.pending.remove(id) }
            FRAME_RELIABLE -> receiveReliable(link, path, id, index, total, payload)
        }
    }

    private fun receiveReliable(
        link: LinkState,
        path: Long,
        id: Int,
        index: Int,
        total: Int,
        payload: ByteArray,
    ) {
        if (total == 0 || index >= total || total > MAX_CONTROL_BYTES / MAX_FRAME_PAYLOAD + 1) return
        val completed = mutableListOf<ByteArray>()
        synchronized(link.receiveLock) {
            val partial = link.partial.getOrPut(id) { PartialMessage(total) }
            if (partial.total != total || partial.parts[index] != null) return
            partial.parts[index] = payload
            partial.received += 1
            partial.bytes += payload.size
            if (partial.bytes > MAX_CONTROL_BYTES) {
                link.partial.remove(id)
                return
            }
            if (partial.received == total) {
                val message = ByteArray(partial.bytes)
                var offset = 0
                partial.parts.forEach { part ->
                    val value = part ?: return
                    value.copyInto(message, offset)
                    offset += value.size
                }
                link.partial.remove(id)
                link.complete[id] = message
                while (true) {
                    val next = link.complete.remove(link.nextInboundReliableId) ?: break
                    completed += next
                    link.nextInboundReliableId += 1
                }
            }
        }
        sendFrame(link, frame(FRAME_ACK, id, 0, 1, ByteArray(0)), path)
        completed.forEach { emit(PlatformEvent.ReliableReceived(path, it)) }
    }

    private fun scheduleRetry(link: LinkState, id: Int, path: Long) {
        handler.postDelayed({
            val pending = synchronized(link.receiveLock) { link.pending[id] }
            if (pending == null) return@postDelayed
            if (pending.attempts >= MAX_RELIABLE_ATTEMPTS) {
                synchronized(link.receiveLock) { link.pending.remove(id) }
                emit(PlatformEvent.PathLost(path, "Wi-Fi Aware control message was not acknowledged"))
                return@postDelayed
            }
            pending.attempts += 1
            if (sendFrames(link, pending.frames, path)) scheduleRetry(link, id, path)
        }, RELIABLE_RETRY_MILLIS)
    }

    private fun sendFrames(link: LinkState, frames: List<ByteArray>, path: Long): Boolean =
        frames.all { sendFrame(link, it, path) }

    private fun sendFrame(link: LinkState, bytes: ByteArray, path: Long): Boolean = try {
        synchronized(link.sendLock) {
            link.session.sendMessage(link.peer, nextMessageId.getAndIncrement(), bytes)
        }
        true
    } catch (error: RuntimeException) {
        Log.w(TAG, "Wi-Fi Aware send failed on path $path", error)
        false
    }

    private fun clearLinks(reason: String) {
        val paths = linksByPath.keys.toList()
        linksByPath.clear()
        linksByAddress.clear()
        addressByPeer.clear()
        paths.forEach { emit(PlatformEvent.PathLost(it, reason)) }
    }

    private fun failAllPaths(reason: String) = clearLinks(reason)

    private companion object {
        const val TAG = "AnvilAware"
        // Keep the Aware service name aligned with the Rust/LAN service name;
        // the transport kind remains in the event, so correlation can fold
        // both sightings into one peer.
        const val SERVICE_NAME = "_anvil._udp"
        const val MAX_ADVERTISEMENT_BYTES = 128
        // Leave headroom below the framework's device-dependent message limit.
        const val MAX_FRAME_PAYLOAD = 180
        const val MAX_DATAGRAM_PAYLOAD = MAX_FRAME_PAYLOAD
        const val MAX_CONTROL_BYTES = 64 * 1024
        const val HEADER_BYTES = 11
        const val MAGIC_0: Byte = 0x41
        const val MAGIC_1: Byte = 0x4e
        const val FRAME_DATAGRAM = 1
        const val FRAME_RELIABLE = 2
        const val FRAME_ACK = 3
        const val MAX_RELIABLE_ATTEMPTS = 3
        const val RELIABLE_RETRY_MILLIS = 350L

        fun frame(type: Int, id: Int, index: Int, total: Int, payload: ByteArray): ByteArray {
            val buffer = ByteBuffer.allocate(HEADER_BYTES + payload.size)
            buffer.put(MAGIC_0)
            buffer.put(MAGIC_1)
            buffer.put(type.toByte())
            buffer.putInt(id)
            buffer.putShort(index.toShort())
            buffer.putShort(total.toShort())
            buffer.put(payload)
            return buffer.array()
        }
    }
}
