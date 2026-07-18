use crate::crypt::{RendezvousInvitation, generate_rendezvous_invitation};

#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
pub fn gen_rdv_inv() -> RendezvousInvitation {
    let (state, inv) = generate_rendezvous_invitation("1".to_string()); //TODO: what should be the id?

    //Do something with state

    inv
}