use crate::crypt::{
    compute_safety_fingerprint, derive_rendezvous_transit_key, generate_rendezvous_invitation,
    RendezvousInvitation, RendezvousState,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};

/// Holds pairing state across the two round trips a rendezvous takes.
/// `pending` covers the gap between generating an invitation and receiving
/// the peer's public key back; `established` keeps the resulting transit
/// key around, ready for the phase 2 bootstrapping exchange once a relay
/// transport exists.
pub struct PairingManager {
    pending: Mutex<HashMap<String, RendezvousState>>,
    established: Mutex<HashMap<String, [u8; 32]>>,
}

impl PairingManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            established: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedRendezvous {
    pub my_pub: [u8; 32],
    pub fingerprint: String,
}

#[derive(Serialize)]
pub struct CompletedRendezvous {
    pub fingerprint: String,
}

fn to_hex_groups(bytes: &[u8]) -> String {
    bytes
        .chunks(2)
        .map(|pair| pair.iter().map(|b| format!("{:02x}", b)).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

fn random_rendezvous_id() -> String {
    let mut rng = rand::rng();
    let mut id_bytes = [0u8; 8];
    rand::RngExt::fill(&mut rng, &mut id_bytes);
    to_hex_groups(&id_bytes).replace(' ', "")
}

#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Initiator side: generate an invitation and hold onto the ephemeral
/// secret until the peer's public key comes back via `complete_rdv_pairing`.
#[tauri::command]
pub fn gen_rdv_inv(manager: State<PairingManager>) -> RendezvousInvitation {
    let rendezvous_id = random_rendezvous_id();
    let (state, invitation) = generate_rendezvous_invitation(rendezvous_id.clone());
    manager.pending.lock().unwrap().insert(rendezvous_id, state);
    invitation
}

/// Responder side: consume a peer's invitation, derive the shared transit
/// key, and hand back this device's public key plus a safety fingerprint
/// the user can compare against the initiator's.
#[tauri::command]
pub fn accept_rdv_inv(
    invitation: RendezvousInvitation,
    manager: State<PairingManager>,
) -> AcceptedRendezvous {
    let mut rng = rand::rng();
    let my_secret = EphemeralSecret::random_from_rng(&mut rng);
    let my_pub = X25519PublicKey::from(&my_secret);

    let transit_key =
        derive_rendezvous_transit_key(my_secret, &invitation.x_temp_pub, &invitation.token);
    let fingerprint = to_hex_groups(&compute_safety_fingerprint(
        my_pub.as_bytes(),
        &invitation.x_temp_pub,
    ));

    manager
        .established
        .lock()
        .unwrap()
        .insert(invitation.rendezvous_id, transit_key);

    AcceptedRendezvous {
        my_pub: *my_pub.as_bytes(),
        fingerprint,
    }
}

/// Initiator side: finish the exchange once the peer's public key is back,
/// deriving the same transit key and fingerprint the responder computed.
#[tauri::command]
pub fn complete_rdv_pairing(
    rendezvous_id: String,
    their_pub: [u8; 32],
    manager: State<PairingManager>,
) -> Result<CompletedRendezvous, String> {
    let state = manager
        .pending
        .lock()
        .unwrap()
        .remove(&rendezvous_id)
        .ok_or("no pending invitation for this rendezvous id")?;

    let my_pub = X25519PublicKey::from(&state.x_temp_sec);
    let transit_key =
        derive_rendezvous_transit_key(state.x_temp_sec, &their_pub, &state.token);
    let fingerprint = to_hex_groups(&compute_safety_fingerprint(my_pub.as_bytes(), &their_pub));

    manager
        .established
        .lock()
        .unwrap()
        .insert(rendezvous_id, transit_key);

    Ok(CompletedRendezvous { fingerprint })
}