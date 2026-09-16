package com.clement.locatorr

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.location.Location
import android.os.Build
import android.util.Log
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/** Never poll faster than this, whatever the caller asks for. */
private const val MIN_INTERVAL_MS = 1_000L

/** Used when the caller sends a missing or nonsensical interval. */
private const val DEFAULT_INTERVAL_MS = 30_000L

/** How long a one-shot fix may take before it gives up. */
private const val ONE_SHOT_TIMEOUT_MS = 15_000L

/**
 * A cached fix older than this is not an answer to "where am I now", so a fresh
 * fix is requested instead. Rust receives `timestamp` either way and can apply
 * its own policy.
 */
private const val LAST_KNOWN_MAX_AGE_MS = 5 * 60 * 1_000L

/**
 * Arguments of `startTracking`.
 *
 * `channel` is nullable rather than `lateinit`: a `lateinit` property cannot be
 * probed with `isInitialized` from another class (the backing field is not
 * accessible), and dereferencing it unchecked would throw instead of rejecting
 * the call. Jackson writes the field directly, so the marker arrives either way.
 */
@InvokeArg
class StartTrackingArgs {
  var intervalMs: Long = DEFAULT_INTERVAL_MS
  var targetName: String? = null
  var channel: Channel? = null
}

/**
 * Process-wide holder for the channel Rust handed over at `startTracking`.
 *
 * The provider and the service run independently of the Activity, so they must
 * never touch the plugin to report something. They talk to this holder instead,
 * and every send is a no-op once the stream is detached, which keeps a stale
 * session from writing to a channel Rust already forgot about.
 */
object LocationStream {
  private const val TAG = "locatorr/android"

  @Volatile
  private var channel: Channel? = null

  /** Attaches [channel]; returns false when a session is already attached. */
  @Synchronized
  fun attach(channel: Channel): Boolean {
    if (this.channel != null) return false
    this.channel = channel
    Log.i(TAG, "location stream attached")
    return true
  }

  @Synchronized
  fun detach() {
    if (channel != null) {
      Log.i(TAG, "location stream detached")
    }
    channel = null
  }

  @Synchronized
  fun isAttached(): Boolean = channel != null

  /** Sends [payload] on the attached channel, or drops it when detached. */
  fun send(payload: JSObject) {
    val target = channel ?: return
    sendTo(target, payload)
  }

  fun sendError(message: String) {
    send(errorPayload(message))
  }

  fun sendPermissionDenied(message: String) {
    send(permissionDeniedPayload(message))
  }

  fun sendStopped(reason: String) {
    send(stoppedPayload(reason))
  }

  /**
   * Sends [payload] on an explicit [channel], swallowing failures.
   *
   * A channel Rust already released must not be able to fail a command.
   */
  fun sendTo(channel: Channel, payload: JSObject) {
    try {
      channel.send(payload)
    } catch (e: Exception) {
      Log.w(TAG, "channel send failed: ${e.message}")
    }
  }

  // The outbound shapes below are a frozen contract with `android_location.rs`.

  fun fixPayload(location: Location): JSObject {
    val payload = JSObject()
    payload.put("kind", "fix")
    payload.put("latitude", location.latitude)
    payload.put("longitude", location.longitude)
    payload.put("accuracy", location.accuracy.toDouble())
    payload.put("timestamp", location.time)
    payload.put("provider", location.provider ?: "unknown")
    return payload
  }

  fun errorPayload(message: String): JSObject {
    val payload = JSObject()
    payload.put("kind", "error")
    payload.put("message", message)
    return payload
  }

  fun permissionDeniedPayload(message: String): JSObject {
    val payload = JSObject()
    payload.put("kind", "permissionDenied")
    payload.put("message", message)
    return payload
  }

  fun stoppedPayload(reason: String): JSObject {
    val payload = JSObject()
    payload.put("kind", "stopped")
    payload.put("reason", reason)
    return payload
  }
}

/**
 * The Android half of the location bridge.
 *
 * Rust drives this through `run_mobile_plugin`, so the command names are the
 * Kotlin method names: `startTracking`, `stopTracking` and `getCurrentPosition`.
 * The inherited `checkPermissions` / `requestPermissions` commands back the
 * permission aliases declared below.
 *
 * Commands run on the main thread and stay short: the long-lived work belongs to
 * [LocationService] and [LocationProvider], both of which are given an
 * application context so no Activity is ever held or touched from a callback.
 */
@TauriPlugin(
  permissions = [
    Permission(
      strings = [
        Manifest.permission.ACCESS_FINE_LOCATION,
        Manifest.permission.ACCESS_COARSE_LOCATION
      ],
      alias = "location"
    ),
    Permission(
      strings = [Manifest.permission.POST_NOTIFICATIONS],
      alias = "notifications"
    )
  ]
)
class LocationPlugin(private val activity: Activity) : Plugin(activity) {

  companion object {
    private const val TAG = "locatorr/android"

    /** Reason reported when the app asks us to stop. */
    private const val STOP_REASON_USER = "user"
  }

  /**
   * Starts the foreground service and attaches the channel fixes will flow to.
   *
   * A second start while a session is attached is a no-op: Tauri never releases a
   * channel id, so starting again would leak the one already in use.
   */
  @Command
  fun startTracking(invoke: Invoke) {
    val args = try {
      invoke.parseArgs(StartTrackingArgs::class.java)
    } catch (e: Exception) {
      invoke.reject("startTracking: could not read the arguments (${e.message})")
      return
    }

    val channel = args.channel
    if (channel == null) {
      invoke.reject("startTracking: the call must include a channel.")
      return
    }

    val context = applicationContext()
    if (context == null) {
      invoke.reject("startTracking: the application context is gone.")
      return
    }

    if (!hasLocationPermission(context)) {
      val message =
        "Location permission is not granted. Grant it from the app's settings, then start tracking again."
      Log.w(TAG, "startTracking refused: no location permission")
      LocationStream.sendTo(channel, LocationStream.permissionDeniedPayload(message))
      invoke.reject(message)
      return
    }

    val providers = LocationProvider(context).availableProviders()
    if (providers.isEmpty()) {
      // Tracking with no provider would run a foreground service that can never
      // produce a fix, so fail loudly instead of starting an idle session.
      val message = "Location is off. Enable GPS or network location, then start tracking again."
      Log.w(TAG, "startTracking refused: no enabled provider")
      invoke.reject(message)
      return
    }

    if (!LocationStream.attach(channel)) {
      Log.i(TAG, "startTracking ignored: a session is already attached")
      invoke.resolve()
      return
    }

    val intervalMs = if (args.intervalMs > 0L) {
      args.intervalMs.coerceAtLeast(MIN_INTERVAL_MS)
    } else {
      DEFAULT_INTERVAL_MS
    }
    val intent = Intent(context, LocationService::class.java).apply {
      putExtra(LocationService.EXTRA_INTERVAL_MS, intervalMs)
      args.targetName?.takeIf { it.isNotBlank() }?.let {
        putExtra(LocationService.EXTRA_TARGET_NAME, it)
      }
    }

    try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        context.startForegroundService(intent)
      } else {
        context.startService(intent)
      }
      Log.i(TAG, "startTracking: service start requested (interval=${intervalMs}ms)")
    } catch (e: Exception) {
      LocationStream.detach()
      val message = "Could not start the tracking service: ${e.message}"
      Log.e(TAG, message, e)
      LocationStream.sendTo(channel, LocationStream.errorPayload(message))
      invoke.reject(message)
      return
    }

    invoke.resolve()
  }

  /**
   * Stops the service and the stream.
   *
   * Idempotent: stopping while nothing runs resolves instead of throwing.
   */
  @Command
  fun stopTracking(invoke: Invoke) {
    val context = applicationContext()
    if (context == null) {
      Log.w(TAG, "stopTracking: no application context; detaching the stream only")
    } else {
      try {
        context.stopService(Intent(context, LocationService::class.java))
        Log.i(TAG, "stopTracking: service stop requested")
      } catch (e: Exception) {
        Log.w(TAG, "stopTracking: stopService failed: ${e.message}")
      }
    }

    // Reported before detaching: a detached stream drops the message.
    LocationStream.sendStopped(STOP_REASON_USER)
    LocationStream.detach()
    invoke.resolve()
  }

  /**
   * Resolves one position.
   *
   * Never throws and never leaves the Invoke hanging: it answers from the cached
   * fix when that fix is recent enough, otherwise it asks for a live fix with a
   * timeout and falls back to the cached one, and only rejects when there is
   * nothing to report at all.
   */
  @Command
  fun getCurrentPosition(invoke: Invoke) {
    val context = applicationContext()
    if (context == null) {
      invoke.reject("The application context is gone, so no position can be read.")
      return
    }

    if (!hasLocationPermission(context)) {
      invoke.reject("Location permission is not granted, so no position can be read.")
      return
    }

    val provider = LocationProvider(context)

    val cached = provider.lastKnownLocation()
    if (cached != null && isFresh(cached)) {
      Log.i(TAG, "getCurrentPosition resolved from a cached fix")
      invoke.resolve(positionPayload(cached))
      return
    }

    val providers = provider.availableProviders()
    if (providers.isEmpty()) {
      invoke.reject("Location is off. Enable GPS or network location and try again.")
      return
    }

    provider.requestSingleFix(ONE_SHOT_TIMEOUT_MS) { location, error ->
      val fix = location ?: provider.lastKnownLocation()
      if (fix != null) {
        invoke.resolve(positionPayload(fix))
      } else {
        invoke.reject(error ?: "No position is available right now.")
      }
    }
  }

  private fun applicationContext(): Context? = activity.applicationContext

  private fun hasLocationPermission(context: Context): Boolean {
    return context.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) ==
      PackageManager.PERMISSION_GRANTED ||
      context.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) ==
      PackageManager.PERMISSION_GRANTED
  }

  private fun isFresh(location: Location): Boolean {
    val age = System.currentTimeMillis() - location.time
    return age in 0..LAST_KNOWN_MAX_AGE_MS
  }

  private fun positionPayload(location: Location): JSObject {
    val payload = JSObject()
    payload.put("latitude", location.latitude)
    payload.put("longitude", location.longitude)
    payload.put("accuracy", location.accuracy.toDouble())
    payload.put("timestamp", location.time)
    return payload
  }
}
