package dev.anvil.anvil

import dev.anvil.AnvilPlatform
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {

    private var anvilPlatform: AnvilPlatform? = null
    private var channel: MethodChannel? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)

        val platform = AnvilPlatform(this)
        anvilPlatform = platform
        channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).apply {
            setMethodCallHandler { call, result ->
                when (call.method) {
                    "attach" -> {
                        val session = call.argument<Number>("session")?.toLong()
                        if (session == null || session == 0L) {
                            result.error("invalid_session", "Missing native session pointer", null)
                        } else {
                            platform.attach(session)
                            result.success(null)
                        }
                    }
                    "detach" -> {
                        platform.detach()
                        result.success(null)
                    }
                    else -> result.notImplemented()
                }
            }
        }
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        channel?.setMethodCallHandler(null)
        channel = null
        anvilPlatform?.detach()
        anvilPlatform = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        anvilPlatform?.onRequestPermissionsResult(requestCode, grantResults)
    }

    private companion object {
        const val CHANNEL = "dev.anvil/platform"
    }
}
