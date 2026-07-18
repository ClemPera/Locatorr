mod crypt;
mod commands;

use commands::PairingManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(PairingManager::new())
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::gen_rdv_inv,
            commands::accept_rdv_inv,
            commands::complete_rdv_pairing
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
