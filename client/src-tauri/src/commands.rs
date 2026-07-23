use crate::crypt::{
    compute_safety_fingerprint, decrypt_identity_bundle, derive_rendezvous_transit_key,
    encrypt_identity_bundle, generate_rendezvous_invitation, ContactStore, IdentityBundle,
    PairedContact, RendezvousInvitation, RendezvousState,
};
use crate::server::ServerClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;
use tokio::time::{sleep, Duration};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret};

pub struct PairingManager {
    pending: Mutex<HashMap<String, RendezvousState>>,
    established: Mutex<HashMap<String, [u8; 32]>>,
    db_path: String,
}

impl PairingManager {
    pub fn new() -> Self {
        let app_dir = std::env::temp_dir();
        let db_path = app_dir.join("locatorr_contacts.sqlite3").to_string_lossy().to_string();
        Self {
            pending: Mutex::new(HashMap::new()),
            established: Mutex::new(HashMap::new()),
            db_path,
        }
    }

    fn open_contacts_db(&self) -> Result<ContactStore, String> {
        ContactStore::open(&self.db_path).map_err(|e| format!("Failed to open contacts DB: {}", e))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedRendezvous {
    pub my_pub: [u8; 32],
    pub fingerprint: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompletedRendezvousWithContact {
    pub fingerprint: String,
    pub peer_device_id: String,
    pub peer_device_name: String,
}

fn to_hex_groups(bytes: &[u8]) -> String {
    bytes
        .chunks(2)
        .map(|pair| pair.iter().map(|b| format!("{:02x}", b)).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

fn get_or_generate_local_identity(device_name: &str) -> IdentityBundle {
    let static_sec = {
        let mut rng = rand::rng();
        StaticSecret::random_from_rng(&mut rng)
    };
    let x25519_pub = X25519PublicKey::from(&static_sec);
    let device_id = to_hex_groups(&x25519_pub.as_bytes()[0..8]).replace(' ', "");

    IdentityBundle {
        device_id,
        device_name: device_name.to_string(),
        x25519_pub: *x25519_pub.as_bytes(),
        ml_kem_pub: vec![],
        ml_dsa_pub: vec![],
    }
}

#[tauri::command]
pub fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Initiator Phase 0 & Phase 1 Step 1: Create room on server, generate invitation,
/// and post initiator's ephemeral pubkey to server.
#[tauri::command]
pub async fn start_pairing_session(
    server_url: String,
    manager: State<'_, PairingManager>,
) -> Result<RendezvousInvitation, String> {
    let server = ServerClient::new(server_url);

    // 1. Create rendezvous room on server
    let room = server.create_rendezvous().await?;
    let rendezvous_id = room.rendezvous_id;

    // 2. Generate local ephemeral keys for rendezvous
    let (state, invitation) = generate_rendezvous_invitation(rendezvous_id.clone());
    let x_temp_pub = invitation.x_temp_pub;

    // 3. Post initiator ephemeral pubkey to server
    server
        .post_pubkey(&rendezvous_id, "initiator", &x_temp_pub)
        .await?;

    // 4. Save state in pending map
    manager
        .pending
        .lock()
        .map_err(|_| "Mutex lock error")?
        .insert(rendezvous_id, state);

    Ok(invitation)
}

/// Initiator Phase 1 Step 2: Poll server for responder's pubkey, complete transit key derivation,
/// encrypt & upload identity bundle, poll for responder's bundle, and save contact.
#[tauri::command]
pub async fn poll_and_complete_pairing(
    server_url: String,
    rendezvous_id: String,
    device_name: String,
    manager: State<'_, PairingManager>,
) -> Result<CompletedRendezvousWithContact, String> {
    let server = ServerClient::new(server_url);

    // 1. Poll for responder's pubkey
    let mut responder_pubkey: Option<Vec<u8>> = None;
    for _ in 0..60 {
        if let Ok(pubkeys) = server.get_pubkeys(&rendezvous_id).await {
            if let Some(resp) = pubkeys.iter().find(|p| p.role == "responder") {
                if resp.pubkey.len() == 32 {
                    responder_pubkey = Some(resp.pubkey.clone());
                    break;
                }
            }
        }
        sleep(Duration::from_millis(1000)).await;
    }

    let responder_pub_bytes: [u8; 32] = responder_pubkey
        .ok_or_else(|| "Timed out waiting for responder to join session".to_string())?
        .try_into()
        .map_err(|_| "Invalid responder pubkey length".to_string())?;

    // 2. Derive transit key & fingerprint
    let state = manager
        .pending
        .lock()
        .map_err(|_| "Mutex lock error")?
        .remove(&rendezvous_id)
        .ok_or_else(|| "No pending invitation found for this session".to_string())?;

    let my_pub = X25519PublicKey::from(&state.x_temp_sec);
    let transit_key =
        derive_rendezvous_transit_key(state.x_temp_sec, &responder_pub_bytes, &state.token);
    let fingerprint =
        to_hex_groups(&compute_safety_fingerprint(my_pub.as_bytes(), &responder_pub_bytes));

    manager
        .established
        .lock()
        .map_err(|_| "Mutex lock error")?
        .insert(rendezvous_id.clone(), transit_key);

    // 3. Encrypt and upload initiator's identity bundle
    let my_identity = get_or_generate_local_identity(&device_name);
    let encrypted_bundle = encrypt_identity_bundle(&transit_key, &my_identity)?;
    server
        .post_bundle(&rendezvous_id, "initiator", &encrypted_bundle)
        .await?;

    // 4. Poll for responder's encrypted identity bundle
    let mut responder_bundle_bytes: Option<Vec<u8>> = None;
    for _ in 0..60 {
        if let Ok(bundles) = server.get_bundles(&rendezvous_id).await {
            if let Some(b) = bundles.iter().find(|b| b.role == "responder") {
                responder_bundle_bytes = Some(b.bundle.clone());
                break;
            }
        }
        sleep(Duration::from_millis(1000)).await;
    }

    let enc_responder_bundle = responder_bundle_bytes
        .ok_or_else(|| "Timed out waiting for responder identity bundle".to_string())?;

    // 5. Decrypt responder bundle and save paired contact
    let peer_bundle = decrypt_identity_bundle(&transit_key, &enc_responder_bundle)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let contact = PairedContact {
        device_id: peer_bundle.device_id.clone(),
        device_name: peer_bundle.device_name.clone(),
        fingerprint: fingerprint.clone(),
        bundle: peer_bundle.clone(),
        paired_at: now,
    };

    let contacts_db = manager.open_contacts_db()?;
    contacts_db
        .save_contact(&contact)
        .map_err(|e| format!("Failed to save paired contact: {}", e))?;

    Ok(CompletedRendezvousWithContact {
        fingerprint,
        peer_device_id: peer_bundle.device_id,
        peer_device_name: peer_bundle.device_name,
    })
}

/// Responder side: Accept invitation, derive transit key, upload pubkey & encrypted bundle,
/// poll for initiator bundle, decrypt and save contact.
#[tauri::command]
pub async fn accept_pairing_session(
    server_url: String,
    invitation: RendezvousInvitation,
    device_name: String,
    manager: State<'_, PairingManager>,
) -> Result<CompletedRendezvousWithContact, String> {
    let server = ServerClient::new(server_url);

    // 1. Generate responder ephemeral keys & derive transit key
    let (my_pub, transit_key, fingerprint) = {
        let mut rng = rand::rng();
        let my_secret = EphemeralSecret::random_from_rng(&mut rng);
        let my_pub = X25519PublicKey::from(&my_secret);
        let transit_key =
            derive_rendezvous_transit_key(my_secret, &invitation.x_temp_pub, &invitation.token);
        let fingerprint =
            to_hex_groups(&compute_safety_fingerprint(my_pub.as_bytes(), &invitation.x_temp_pub));
        (my_pub, transit_key, fingerprint)
    };

    manager
        .established
        .lock()
        .map_err(|_| "Mutex lock error")?
        .insert(invitation.rendezvous_id.clone(), transit_key);

    // 2. Post responder ephemeral pubkey to server
    server
        .post_pubkey(&invitation.rendezvous_id, "responder", my_pub.as_bytes())
        .await?;

    // 3. Encrypt and post responder identity bundle to server
    let my_identity = get_or_generate_local_identity(&device_name);
    let encrypted_bundle = encrypt_identity_bundle(&transit_key, &my_identity)?;
    server
        .post_bundle(&invitation.rendezvous_id, "responder", &encrypted_bundle)
        .await?;

    // 4. Poll for initiator's encrypted identity bundle
    let mut initiator_bundle_bytes: Option<Vec<u8>> = None;
    for _ in 0..60 {
        if let Ok(bundles) = server.get_bundles(&invitation.rendezvous_id).await {
            if let Some(b) = bundles.iter().find(|b| b.role == "initiator") {
                initiator_bundle_bytes = Some(b.bundle.clone());
                break;
            }
        }
        sleep(Duration::from_millis(1000)).await;
    }

    let enc_initiator_bundle = initiator_bundle_bytes
        .ok_or_else(|| "Timed out waiting for initiator identity bundle".to_string())?;

    // 5. Decrypt initiator bundle and save paired contact
    let peer_bundle = decrypt_identity_bundle(&transit_key, &enc_initiator_bundle)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let contact = PairedContact {
        device_id: peer_bundle.device_id.clone(),
        device_name: peer_bundle.device_name.clone(),
        fingerprint: fingerprint.clone(),
        bundle: peer_bundle.clone(),
        paired_at: now,
    };

    let contacts_db = manager.open_contacts_db()?;
    contacts_db
        .save_contact(&contact)
        .map_err(|e| format!("Failed to save paired contact: {}", e))?;

    Ok(CompletedRendezvousWithContact {
        fingerprint,
        peer_device_id: peer_bundle.device_id,
        peer_device_name: peer_bundle.device_name,
    })
}

/// Legacy command kept for compatibility
#[tauri::command]
pub fn gen_rdv_inv(manager: State<'_, PairingManager>) -> RendezvousInvitation {
    let mut rng = rand::rng();
    let mut id_bytes = [0u8; 8];
    rand::RngExt::fill(&mut rng, &mut id_bytes);
    let rendezvous_id = to_hex_groups(&id_bytes).replace(' ', "");

    let (state, invitation) = generate_rendezvous_invitation(rendezvous_id.clone());
    manager.pending.lock().unwrap().insert(rendezvous_id, state);
    invitation
}

/// Legacy command kept for compatibility
#[tauri::command]
pub fn accept_rdv_inv(
    invitation: RendezvousInvitation,
    manager: State<'_, PairingManager>,
) -> AcceptedRendezvous {
    let mut rng = rand::rng();
    let my_secret = EphemeralSecret::random_from_rng(&mut rng);
    let my_pub = X25519PublicKey::from(&my_secret);

    let transit_key =
        derive_rendezvous_transit_key(my_secret, &invitation.x_temp_pub, &invitation.token);
    let fingerprint =
        to_hex_groups(&compute_safety_fingerprint(my_pub.as_bytes(), &invitation.x_temp_pub));

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

/// Legacy command kept for compatibility
#[tauri::command]
pub fn complete_rdv_pairing(
    rendezvous_id: String,
    their_pub: [u8; 32],
    manager: State<'_, PairingManager>,
) -> Result<crate::commands::CompletedRendezvous, String> {
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

    Ok(crate::commands::CompletedRendezvous { fingerprint })
}

#[derive(Serialize)]
pub struct CompletedRendezvous {
    pub fingerprint: String,
}

#[tauri::command]
pub fn get_paired_contacts(
    manager: State<'_, PairingManager>,
) -> Result<Vec<PairedContact>, String> {
    let db = manager.open_contacts_db()?;
    db.get_contacts().map_err(|e| format!("Failed to read contacts: {}", e))
}

#[tauri::command]
pub fn delete_paired_contact(
    device_id: String,
    manager: State<'_, PairingManager>,
) -> Result<(), String> {
    let db = manager.open_contacts_db()?;
    db.delete_contact(&device_id).map_err(|e| format!("Failed to delete contact: {}", e))
}