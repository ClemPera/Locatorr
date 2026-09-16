mod android_location;
mod crypt;
mod server;
mod commands;

use commands::{PairingManager, TrackingState};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(android_location::init())
        .setup(|app| {
            // The identity and sequence databases must survive restarts, so they
            // live in the app data dir rather than a temp dir.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(PairingManager::new(data_dir.to_string_lossy().to_string()));
            app.manage(TrackingState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::gen_rdv_inv,
            commands::accept_rdv_inv,
            commands::complete_rdv_pairing,
            commands::start_pairing_session,
            commands::poll_and_complete_pairing,
            commands::accept_pairing_session,
            commands::get_paired_contacts,
            commands::delete_paired_contact,
            commands::send_location_update,
            commands::poll_location_updates,
            commands::start_tracking,
            commands::stop_tracking,
            commands::get_current_position,
            commands::get_received_updates
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                // Tauri raises this when the window/webview set becomes empty
                // (on Android: the activity is destroyed). While tracking, the
                // Kotlin foreground service is still sending, so tearing Rust
                // down would silently stop it. When idle, a normal quit must
                // still work.
                if app.state::<TrackingState>().is_running() {
                    api.prevent_exit();
                }
            }
        });
}
