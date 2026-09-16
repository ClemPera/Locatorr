package com.clement.locatorr

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * The foreground service that keeps background tracking legal and visible.
 *
 * It owns the location cadence and the notification, and nothing else: policy
 * lives in Rust. It is always started from a visible Activity (the `startTracking`
 * plugin command), which is why the manifest asks for no background location
 * permission.
 *
 * Returns [START_NOT_STICKY]: a framework restart of a `location` foreground
 * service is not allowed from the background, so a sticky service would just be
 * a crash loop.
 */
class LocationService : Service() {

  companion object {
    private const val TAG = "locatorr/android"

    /** Action of the notification's Stop button; the intent is explicit, so this is private to us. */
    const val ACTION_STOP = "stop"

    /** Extras of the start intent. */
    const val EXTRA_INTERVAL_MS = "intervalMs"
    const val EXTRA_TARGET_NAME = "targetName"

    private const val CHANNEL_ID = "locatorr-location"
    private const val CHANNEL_NAME = "Location sharing"
    private const val CHANNEL_DESCRIPTION = "Shown while Locatorr shares your location."
    private const val NOTIFICATION_ID = 4211
    private const val STOP_REQUEST_CODE = 4212

    /** Reason reported to Rust when the notification's Stop button ends the session. */
    private const val STOP_REASON_NOTIFICATION = "notification"

    private const val DEFAULT_INTERVAL_MS = 30_000L
    private const val MIN_INTERVAL_MS = 1_000L
  }

  private var provider: LocationProvider? = null

  /**
   * True once [startForeground] succeeded; guards against a duplicate start
   * re-registering the location listeners.
   */
  private var started = false

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    Log.i(TAG, "service created")
    // The channel must exist before the notification is posted.
    ensureNotificationChannel()
    provider = LocationProvider(applicationContext)
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_STOP) {
      Log.i(TAG, "stop action received from the notification")
      shutdown(STOP_REASON_NOTIFICATION)
      return START_NOT_STICKY
    }

    if (intent == null) {
      Log.w(TAG, "started without an intent; nothing to track")
      shutdown(null)
      return START_NOT_STICKY
    }

    val intervalMs = intent.getLongExtra(EXTRA_INTERVAL_MS, 0L)
      .let { if (it > 0L) it.coerceAtLeast(MIN_INTERVAL_MS) else DEFAULT_INTERVAL_MS }
    val targetName = intent.getStringExtra(EXTRA_TARGET_NAME)

    if (started) {
      Log.i(TAG, "already tracking; ignoring the duplicate start")
      return START_NOT_STICKY
    }

    warnIfNotificationsAreMuted()

    // Android requires the service to be in the foreground within a few seconds of
    // startForegroundService, so this happens before any location work.
    if (!promoteToForeground(targetName)) {
      // The failure was already reported over the channel.
      return START_NOT_STICKY
    }

    started = true
    provider?.start(intervalMs)

    return START_NOT_STICKY
  }

  override fun onDestroy() {
    Log.i(TAG, "service destroyed")
    started = false
    provider?.stop()
    LocationStream.detach()
    super.onDestroy()
  }

  /**
   * Enters the foreground with the `location` type.
   *
   * Returns false (after reporting and shutting down) when Android refuses, which
   * happens if the type is not granted or the start was not allowed from the
   * current state. Crashing here would take the app down with it.
   */
  private fun promoteToForeground(targetName: String?): Boolean {
    val notification = buildNotification(targetName)
    return try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        startForeground(
          NOTIFICATION_ID,
          notification,
          ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION
        )
      } else {
        startForeground(NOTIFICATION_ID, notification)
      }
      Log.i(TAG, "entered the foreground with the location type")
      true
    } catch (e: SecurityException) {
      failToForeground("the system rejected the foreground service (${e.message ?: "SecurityException"})")
      false
    } catch (e: Exception) {
      failToForeground("the system rejected the foreground service (${e.message ?: e.javaClass.simpleName})")
      false
    }
  }

  /** Reports a foreground-service failure over the channel and tears the service down. */
  private fun failToForeground(message: String) {
    Log.e(TAG, message)
    LocationStream.sendError("Location tracking could not start: $message")
    shutdown(null)
  }

  /**
   * Ends the session.
   *
   * [reason] is only sent when it is non-null, so a stop is reported over the
   * channel exactly once, by whoever started the shutdown.
   */
  private fun shutdown(reason: String?) {
    if (reason != null) {
      LocationStream.sendStopped(reason)
    }
    started = false
    provider?.stop()
    LocationStream.detach()
    try {
      stopForeground(Service.STOP_FOREGROUND_REMOVE)
    } catch (e: Exception) {
      Log.w(TAG, "stopForeground failed: ${e.message}")
    }
    stopSelf()
  }

  private fun buildNotification(targetName: String?): Notification {
    val stopIntent = Intent(this, LocationService::class.java).setAction(ACTION_STOP)
    val stopPendingIntent = PendingIntent.getService(
      this,
      STOP_REQUEST_CODE,
      stopIntent,
      PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
    )

    val title = if (targetName.isNullOrBlank()) {
      "Sharing your location"
    } else {
      "Sharing location with $targetName"
    }

    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.drawable.ic_launcher_foreground)
      .setContentTitle(title)
      .setContentText("Your location is being shared until you stop tracking.")
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setShowWhen(false)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setVisibility(NotificationCompat.VISIBILITY_SECRET)
      .addAction(0, "Stop", stopPendingIntent)
      .build()
  }

  private fun ensureNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return

    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
    if (manager.getNotificationChannel(CHANNEL_ID) != null) return

    val channel = NotificationChannel(
      CHANNEL_ID,
      CHANNEL_NAME,
      NotificationManager.IMPORTANCE_LOW
    ).apply {
      description = CHANNEL_DESCRIPTION
      setShowBadge(false)
      enableVibration(false)
      setSound(null, null)
      enableLights(false)
    }
    manager.createNotificationChannel(channel)
    Log.i(TAG, "created the notification channel $CHANNEL_ID")
  }

  /**
   * POST_NOTIFICATIONS is a runtime permission from API 33 on. When it is denied
   * the service still runs and the notification is simply not shown in the
   * drawer, so this only logs.
   */
  private fun warnIfNotificationsAreMuted() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    val granted = checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
      PackageManager.PERMISSION_GRANTED
    if (!granted) {
      Log.w(TAG, "POST_NOTIFICATIONS is not granted; tracking runs without a visible notification")
    }
  }
}
