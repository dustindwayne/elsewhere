package com.dustindwayne.elsewhere

import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private var nearbyLock: android.net.wifi.WifiManager.MulticastLock? = null
  override fun onResume() {
    super.onResume()
    val wifi = applicationContext.getSystemService(android.content.Context.WIFI_SERVICE) as? android.net.wifi.WifiManager
    nearbyLock = wifi?.createMulticastLock("elsewhere-nearby")?.apply { setReferenceCounted(false); acquire() }
  }
  override fun onPause() {
    nearbyLock?.let { if (it.isHeld) it.release() }
    nearbyLock = null
    super.onPause()
  }
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    val content = findViewById<android.view.View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      insets
    }
  }
}
