package com.clement.locatorr

import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // LocationService is not auto-started — it requires runtime permissions
    // and should only start when the user explicitly enables background sharing.
    // Foreground location is handled by tauri-plugin-geolocation.
  }
}
