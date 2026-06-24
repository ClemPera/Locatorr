mod commands;
mod db;
mod relay;

use std::sync::Mutex;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open_and_migrate(&data_dir.join("locatorr.sqlite"))?;
            app.manage(AppState {
                conn: Mutex::new(conn),
                relay_user_id: Mutex::new(String::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_pairing_payload,
            commands::add_contact,
            commands::list_contacts,
            commands::check_contact_fingerprint,
            commands::verify_contact,
            commands::set_contact_sharing,
            commands::remove_contact,
            commands::get_settings,
            commands::update_settings,
            commands::list_received_locations,
            commands::register_with_relay,
            commands::authenticate_with_relay,
            commands::send_location_update,
            commands::poll_inbox_for_locations,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
