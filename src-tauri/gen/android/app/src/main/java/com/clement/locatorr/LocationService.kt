package com.clement.locatorr

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Bundle
import android.os.IBinder

class LocationService : Service(), LocationListener {

    private lateinit var locationManager: LocationManager

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel(
            "location",
            "Location Sharing",
            NotificationManager.IMPORTANCE_LOW
        )
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)

        val launch = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE
        )
        startForeground(1, Notification.Builder(this, "location")
            .setContentTitle("Locatorr")
            .setContentText("Sharing your location")
            .setSmallIcon(android.R.drawable.ic_menu_compass)
            .setContentIntent(launch)
            .build())

        locationManager = getSystemService(LOCATION_SERVICE) as LocationManager
        try {
            locationManager.requestLocationUpdates(
                LocationManager.GPS_PROVIDER, 10000, 5f, this
            )
        } catch (_: SecurityException) {}
    }

    override fun onLocationChanged(loc: Location) {
        sendBroadcast(Intent("com.clement.locatorr.LOCATION").apply {
            putExtra("lat", loc.latitude)
            putExtra("lon", loc.longitude)
            putExtra("accuracy", loc.accuracy.toDouble())
        })
    }

    override fun onDestroy() {
        locationManager.removeUpdates(this)
        super.onDestroy()
    }

    override fun onProviderDisabled(provider: String) {}
    override fun onProviderEnabled(provider: String) {}
    override fun onStatusChanged(provider: String, status: Int, extras: Bundle?) {}
}
