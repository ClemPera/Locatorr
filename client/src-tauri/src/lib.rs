mod crypt;
mod server;
mod commands;

use commands::PairingManager;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // The identity and sequence databases must survive restarts, so they
            // live in the app data dir rather than a temp dir.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(PairingManager::new(data_dir.to_string_lossy().to_string()));
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
            commands::poll_location_updates
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
