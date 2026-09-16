//! Android geolocation bridge.
//!
//! Kotlin owns the `LocationManager` cadence, the foreground service and the
//! notification. Rust owns policy: it reacts to the fixes Kotlin streams over
//! the channel and decides what (if anything) to send.
//!
//! The Kotlin class lives in `gen/android/.../LocationPlugin.kt`; the method
//! names and channel message shapes are a frozen contract.

use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

#[cfg(target_os = "android")]
use serde::Serialize;
#[cfg(not(target_os = "android"))]
use std::marker::PhantomData;
#[cfg(target_os = "android")]
use std::sync::Mutex;
#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

/// Returned by every bridge call on a non-Android build. Dead code on Android,
/// where the real implementation is compiled instead.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub const ANDROID_ONLY: &str = "background tracking is Android-only";

/// What Kotlin's `getCurrentPosition` resolves with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePosition {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres. Negative means "unknown" (Kotlin's own sentinel).
    pub accuracy: f32,
    /// Milliseconds since the Unix epoch, straight from `Location.getTime()`.
    pub timestamp: i64,
}

/// Messages Kotlin pushes over the tracking channel, discriminated by `kind`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrackingEvent {
    Fix {
        latitude: f64,
        longitude: f64,
        accuracy: f32,
        timestamp: i64,
        /// Part of the frozen Kotlin contract; not shown anywhere (yet), so it
        /// is explicitly allowed to be unread rather than deleted.
        #[allow(dead_code)]
        provider: String,
    },
    Error {
        message: String,
    },
    PermissionDenied {
        message: String,
    },
    Stopped {
        reason: String,
    },
}

#[cfg(target_os = "android")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartTrackingPayload {
    interval_ms: u64,
    /// Shown in the always-visible foreground notification ("Sharing location
    /// with <name>"); Kotlin falls back to a generic title when empty.
    target_name: String,
    channel: Channel<serde_json::Value>,
}

pub struct AndroidLocation<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: PluginHandle<R>,
    /// The channel for the active session, if any. Tauri never unregisters a
    /// channel id, so keeping it here is what lets a second `startTracking` be
    /// a no-op instead of leaking a channel Kotlin would keep writing to.
    #[cfg(target_os = "android")]
    channel: Mutex<Option<Channel<serde_json::Value>>>,
    #[cfg(not(target_os = "android"))]
    _marker: PhantomData<fn() -> R>,
}

#[cfg(target_os = "android")]
impl<R: Runtime> AndroidLocation<R> {
    pub fn new(handle: PluginHandle<R>) -> Self {
        Self {
            handle,
            channel: Mutex::new(None),
        }
    }

    /// True while Kotlin has a tracking session we started.
    pub fn is_active(&self) -> bool {
        self.channel
            .lock()
            .map(|channel| channel.is_some())
            .unwrap_or(false)
    }

    /// Guards every call into Kotlin. Do not remove this check.
    ///
    /// `run_mobile_plugin*` dispatches through `wry::android::dispatch`, which
    /// panics in `first_activity_id().expect("no available activity")` once the
    /// activity proxy has been removed (activity destroyed / window closed).
    /// The release profile is `panic = "unwind"` precisely so a panic on the
    /// channel path cannot abort the process, but a panic here would still
    /// poison the plugin call, so we refuse to call at all when no window
    /// exists. `webview_windows` is empty exactly when the proxy is gone.
    fn ensure_window(&self) -> Result<(), String> {
        if self.handle.app().webview_windows().is_empty() {
            return Err(
                "no window available: the Android activity is gone, refusing to call the location plugin"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub async fn start(
        &self,
        interval_ms: u64,
        target_name: &str,
        channel: Channel<serde_json::Value>,
    ) -> Result<(), String> {
        if self.is_active() {
            // Already tracking: no-op rather than leaking a second channel id.
            return Ok(());
        }

        self.ensure_window()?;

        let payload = StartTrackingPayload {
            interval_ms,
            target_name: target_name.to_string(),
            channel: channel.clone(),
        };
        self.handle
            .run_mobile_plugin_async::<()>("startTracking", payload)
            .await
            .map_err(|e| format!("startTracking failed: {}", e))?;

        self.replace_channel(Some(channel))
    }

    pub async fn stop(&self) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }

        self.ensure_window()?;
        self.handle
            .run_mobile_plugin_async::<()>("stopTracking", ())
            .await
            .map_err(|e| format!("stopTracking failed: {}", e))?;

        self.replace_channel(None)
    }

    pub async fn get_current_position(&self) -> Result<NativePosition, String> {
        self.ensure_window()?;
        self.handle
            .run_mobile_plugin_async::<NativePosition>("getCurrentPosition", ())
            .await
            .map_err(|e| format!("getCurrentPosition failed: {}", e))
    }

    fn replace_channel(&self, channel: Option<Channel<serde_json::Value>>) -> Result<(), String> {
        let mut slot = self
            .channel
            .lock()
            .map_err(|_| "location plugin state poisoned".to_string())?;
        *slot = channel;
        Ok(())
    }
}

#[cfg(not(target_os = "android"))]
impl<R: Runtime> AndroidLocation<R> {
    pub fn new_disabled() -> Self {
        Self {
            _marker: PhantomData,
        }
    }

    pub fn is_active(&self) -> bool {
        false
    }

    pub async fn start(
        &self,
        _interval_ms: u64,
        _target_name: &str,
        _channel: Channel<serde_json::Value>,
    ) -> Result<(), String> {
        Err(ANDROID_ONLY.to_string())
    }

    pub async fn stop(&self) -> Result<(), String> {
        Err(ANDROID_ONLY.to_string())
    }

    pub async fn get_current_position(&self) -> Result<NativePosition, String> {
        Err(ANDROID_ONLY.to_string())
    }
}

/// Registers the native `LocationPlugin` and manages the Rust-side bridge.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("android-location")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin("com.clement.locatorr", "LocationPlugin")?;
                app.manage(AndroidLocation::<R>::new(handle));
            }

            #[cfg(not(target_os = "android"))]
            {
                // Managed on every platform so the tracking commands can run
                // (and return ANDROID_ONLY) instead of failing a state lookup.
                let _ = &api;
                app.manage(AndroidLocation::<R>::new_disabled());
            }

            Ok(())
        })
        .build()
}
