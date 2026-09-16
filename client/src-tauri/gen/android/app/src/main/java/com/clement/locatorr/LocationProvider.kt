package com.clement.locatorr

import android.content.Context
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log

/**
 * Thin wrapper around the framework [LocationManager].
 *
 * Only the platform API is used on purpose: `play-services-location` would add a
 * dependency and a Google Play requirement, while [LocationManager] is present on
 * every device this app can be installed on.
 *
 * Every call here is defensive. [LocationManager] throws
 * - `SecurityException` when the permission was revoked between the check and the
 *   call, and
 * - `IllegalArgumentException` when the provider is not enabled (which is exactly
 *   what `getProviders(true)` exists to avoid).
 *
 * Nothing in this class touches an Activity: the caller passes an application
 * context, and fixes leave through [LocationStream] so a destroyed Activity can
 * never be reached from a location callback.
 */
class LocationProvider(private val context: Context) {

  companion object {
    private const val TAG = "locatorr/android"

    /**
     * The providers we are willing to register for, in preference order.
     *
     * `FUSED_PROVIDER` is deliberately absent: it is deprecated and not present on
     * every device, and registering against a missing provider throws.
     */
    private val CANDIDATE_PROVIDERS = listOf(
      LocationManager.GPS_PROVIDER,
      LocationManager.NETWORK_PROVIDER,
      LocationManager.PASSIVE_PROVIDER
    )
  }

  private val locationManager: LocationManager? =
    context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager

  private val mainLooper: Looper = Looper.getMainLooper()

  /** One listener for the streaming session, so [stop] removes exactly what [start] added. */
  private val streamingListener = object : LocationListener {
    override fun onLocationChanged(location: Location) {
      emit(location)
    }

    override fun onProviderEnabled(provider: String) {
      Log.i(TAG, "provider enabled: $provider")
    }

    override fun onProviderDisabled(provider: String) {
      Log.i(TAG, "provider disabled: $provider")
    }

    @Deprecated("Deprecated in Java")
    override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) {
      Log.d(TAG, "provider status changed: $provider ($status)")
    }
  }

  private var streaming = false

  /**
   * The enabled providers out of [CANDIDATE_PROVIDERS].
   *
   * `getProviders(true)` lists every provider that exists *and* is enabled, so an
   * empty result means location is off (or every provider was disabled by policy).
   * Never throws: a failure is logged and reported as "nothing is available".
   */
  fun availableProviders(): List<String> {
    val manager = locationManager ?: return emptyList()
    return try {
      // Typed nullable: the platform stub promises non-null, but a null list would
      // otherwise blow up inside filter() rather than degrade to "nothing enabled".
      val enabled: List<String>? = manager.getProviders(true)
      if (enabled == null || enabled.isEmpty()) return emptyList()
      CANDIDATE_PROVIDERS.filter { enabled.contains(it) }
    } catch (e: SecurityException) {
      Log.w(TAG, "cannot list location providers: ${e.message}")
      emptyList()
    } catch (e: Exception) {
      Log.w(TAG, "cannot list location providers: ${e.message}")
      emptyList()
    }
  }

  /**
   * The freshest cached fix we can get without waiting, or null when there is none.
   *
   * Android's "last known location" can be arbitrarily old, so callers decide
   * whether its timestamp is good enough to use.
   */
  fun lastKnownLocation(): Location? {
    val manager = locationManager ?: return null
    var best: Location? = null
    for (provider in availableProviders()) {
      val candidate = try {
        manager.getLastKnownLocation(provider)
      } catch (e: SecurityException) {
        reportPermissionDenied("cannot read the last known location: ${e.message}")
        null
      } catch (e: Exception) {
        Log.w(TAG, "no last known location from $provider: ${e.message}")
        null
      }
      if (candidate == null) continue
      val current = best
      if (current == null || candidate.time > current.time) {
        best = candidate
      }
    }
    return best
  }

  /** Starts streaming fixes every [intervalMs] to the attached [LocationStream]. */
  fun start(intervalMs: Long) {
    if (streaming) {
      Log.i(TAG, "already streaming; ignoring start")
      return
    }

    val manager = locationManager
    if (manager == null) {
      reportError("This device has no LocationManager, so tracking cannot start.")
      return
    }

    val providers = availableProviders()
    if (providers.isEmpty()) {
      reportError("Location is off: neither GPS nor network location is enabled.")
      return
    }

    // Marked before registering so a failure below cannot leave the flag lying.
    streaming = true

    for (provider in providers) {
      try {
        // The 4-argument overload resolves a Looper from the calling thread and
        // throws when there is none, so the main looper is always passed explicitly.
        manager.requestLocationUpdates(provider, intervalMs, 0f, streamingListener, mainLooper)
        Log.i(TAG, "requested updates from $provider every ${intervalMs}ms")
      } catch (e: SecurityException) {
        reportPermissionDenied("location permission was revoked: ${e.message}")
      } catch (e: IllegalArgumentException) {
        Log.w(TAG, "provider $provider is not available: ${e.message}")
      } catch (e: Exception) {
        reportError("Could not request updates from $provider: ${e.message}")
      }
    }
  }

  /** Removes every listener this provider registered. Safe to call twice. */
  fun stop() {
    streaming = false
    val manager = locationManager ?: return
    try {
      manager.removeUpdates(streamingListener)
      Log.i(TAG, "stopped listening for location updates")
    } catch (e: SecurityException) {
      reportPermissionDenied("cannot remove location updates: ${e.message}")
    } catch (e: Exception) {
      reportError("cannot remove location updates: ${e.message}")
    }
  }

  /**
   * Asks for one fix.
   *
   * [onResult] is invoked **exactly once**, on the main thread, with either a fix
   * or a human-readable error - never both, never neither. It is also invoked
   * when no provider is usable, so the caller never has to wait for the timeout.
   */
  fun requestSingleFix(timeoutMs: Long, onResult: (location: Location?, error: String?) -> Unit) {
    val manager = locationManager
    if (manager == null) {
      onResult(null, "This device has no LocationManager, so it cannot resolve a position.")
      return
    }

    val providers = availableProviders()
    if (providers.isEmpty()) {
      onResult(null, "Location is off: enable GPS or network location and try again.")
      return
    }

    val handler = Handler(mainLooper)
    var finished = false
    var activeListener: LocationListener? = null
    var timeout: Runnable? = null

    fun finish(location: Location?, error: String?) {
      if (finished) return
      finished = true
      timeout?.let { handler.removeCallbacks(it) }
      activeListener?.let { listener ->
        try {
          manager.removeUpdates(listener)
        } catch (e: Exception) {
          Log.w(TAG, "cannot remove the one-shot listener: ${e.message}")
        }
      }
      if (location != null) {
        Log.i(TAG, "one-shot fix from ${location.provider ?: "unknown"}")
      } else {
        Log.w(TAG, "one-shot fix failed: $error")
      }
      onResult(location, error)
    }

    val listener = object : LocationListener {
      override fun onLocationChanged(location: Location) {
        finish(location, null)
      }

      override fun onProviderEnabled(provider: String) {}

      override fun onProviderDisabled(provider: String) {}

      @Deprecated("Deprecated in Java")
      override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) {}
    }

    val timeoutRunnable = Runnable {
      finish(null, "No location fix arrived within ${timeoutMs / 1000} seconds.")
    }
    timeout = timeoutRunnable

    var requested = 0
    for (provider in providers) {
      try {
        manager.requestLocationUpdates(provider, 0L, 0f, listener, mainLooper)
        requested++
        Log.d(TAG, "one-shot: listening to $provider")
      } catch (e: SecurityException) {
        reportPermissionDenied("location permission was revoked: ${e.message}")
      } catch (e: IllegalArgumentException) {
        Log.w(TAG, "one-shot: provider $provider is not available: ${e.message}")
      } catch (e: Exception) {
        reportError("Could not request a fix from $provider: ${e.message}")
      }
    }

    if (requested == 0) {
      finish(null, "No location provider could be used to resolve a position.")
      return
    }

    activeListener = listener
    handler.postDelayed(timeoutRunnable, timeoutMs.coerceAtLeast(1_000L))
  }

  /** Sends one fix over [LocationStream] and logs which provider produced it. */
  private fun emit(location: Location) {
    val providerName = location.provider ?: "unknown"
    val ageMs = System.currentTimeMillis() - location.time
    Log.d(TAG, "fix from $providerName: accuracy=${location.accuracy}m age=${ageMs}ms")
    LocationStream.send(LocationStream.fixPayload(location))
  }

  private fun reportError(message: String) {
    Log.w(TAG, message)
    LocationStream.sendError(message)
  }

  private fun reportPermissionDenied(message: String) {
    Log.w(TAG, message)
    LocationStream.sendPermissionDenied(message)
  }
}
