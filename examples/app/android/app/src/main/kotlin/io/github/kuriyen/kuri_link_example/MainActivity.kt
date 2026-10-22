package io.github.kuriyen.kuri_link_example

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

// Android tells Java code, not native code, when the network changes. So the
// activity watches the default network and reports each change to Dart, which
// passes it on to the Kuri Link node.
//
// When Dart asks, because the node uses mDNS, it also holds a multicast lock
// while the app runs. Without one, Android drops multicast packets before they
// reach the app, so the node cannot hear the back end announce itself on the
// home network, and cannot find it there once its address changed.
class MainActivity : FlutterActivity() {
    private var callback: ConnectivityManager.NetworkCallback? = null
    private var multicast: WifiManager.MulticastLock? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, NETWORK_CHANNEL)
        val main = Handler(Looper.getMainLooper())
        val changed = { main.post { channel.invokeMethod("changed", null) } }
        val watch = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                changed()
            }

            override fun onLost(network: Network) {
                changed()
            }

            override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
                changed()
            }
        }
        connectivity().registerDefaultNetworkCallback(watch)
        callback = watch

        channel.setMethodCallHandler { call, result ->
            if (call.method == "holdMulticastLock") {
                holdMulticastLock()
                result.success(null)
            } else {
                result.notImplemented()
            }
        }
    }

    private fun holdMulticastLock() {
        if (multicast != null) return
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock(MULTICAST_LOCK).apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        callback?.let { connectivity().unregisterNetworkCallback(it) }
        callback = null
        multicast?.release()
        multicast = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    private fun connectivity() =
        getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager

    private companion object {
        const val NETWORK_CHANNEL = "kuri_link_example/network"
        const val MULTICAST_LOCK = "kuri_link_example/mdns"
    }
}
