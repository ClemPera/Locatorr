use crate::crypt::{
    compute_safety_fingerprint, decrypt_identity_bundle, derive_rendezvous_transit_key,
    encrypt_identity_bundle, generate_rendezvous_invitation, to_hex_groups, ContactStore,
    LocationPayload, PairedContact, RendezvousInvitation, RendezvousState, SequenceStore,
};
use crate::server::ServerClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;
use tokio::time::{sleep, Duration};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};

pub struct PairingManager {
    pending: Mutex<HashMap<String, RendezvousState>>,
    established: Mutex<HashMap<String, [u8; 32]>>,
    data_dir: String,
}

impl PairingManager {
    pub fn new(data_dir: String) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            established: Mutex::new(HashMap::new()),
            data_dir,
        }
    }

    fn contacts_db_path(&self) -> String {
        std::path::Path::new(&self.data_dir)
            .join("locatorr.sqlite3")
            .to_string_lossy()
            .to_string()
    }

    fn sequence_db_path(&self) -> String {
        std::path::Path::new(&self.data_dir)
            .join("locatorr_sequence.sqlite3")
            .to_string_lossy()
            .to_string()
    }

    fn open_contacts_db(&self) -> Result<ContactStore, String> {
        ContactStore::open(&self.contacts_db_path()).map_err(|e| format!("Failed to open contacts DB: {}", e))
    }

    fn open_sequence_store(&self) -> Result<SequenceStore, String> {
        SequenceStore::open(&self.sequence_db_path())
            .map_err(|e| format!("Failed to open sequence DB: {}", e))
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

    // 2. Derive transit key
    let state = manager
        .pending
        .lock()
        .map_err(|_| "Mutex lock error")?
        .remove(&rendezvous_id)
        .ok_or_else(|| "No pending invitation found for this session".to_string())?;

    let transit_key =
        derive_rendezvous_transit_key(state.x_temp_sec, &responder_pub_bytes, &state.token);

    manager
        .established
        .lock()
        .map_err(|_| "Mutex lock error")?
        .insert(rendezvous_id.clone(), transit_key);

    // 3. Encrypt and upload initiator's identity bundle
    let identity = manager
        .open_contacts_db()?
        .load_or_create_identity(&device_name)
        .map_err(|e| format!("Failed to load local identity: {}", e))?;
    let encrypted_bundle = encrypt_identity_bundle(&transit_key, &identity.bundle)?;
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

    // 5. Compute safety fingerprint over both encrypted bundles (protocol step 6)
    let fingerprint =
        to_hex_groups(&compute_safety_fingerprint(&encrypted_bundle, &enc_responder_bundle));

    // 6. Decrypt responder bundle and save paired contact
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
    let (my_pub, transit_key) = {
        let mut rng = rand::rng();
        let my_secret = EphemeralSecret::random_from_rng(&mut rng);
        let my_pub = X25519PublicKey::from(&my_secret);
        let transit_key =
            derive_rendezvous_transit_key(my_secret, &invitation.x_temp_pub, &invitation.token);
        (my_pub, transit_key)
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
    let identity = manager
        .open_contacts_db()?
        .load_or_create_identity(&device_name)
        .map_err(|e| format!("Failed to load local identity: {}", e))?;
    let encrypted_bundle = encrypt_identity_bundle(&transit_key, &identity.bundle)?;
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

    // 5. Compute safety fingerprint over both encrypted bundles (protocol step 6)
    let fingerprint =
        to_hex_groups(&compute_safety_fingerprint(&encrypted_bundle, &enc_initiator_bundle));

    // 6. Decrypt initiator bundle and save paired contact
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

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SentLocationUpdate {
    pub contact_device_id: String,
    pub contact_device_name: String,
    pub sequence: u64,
    pub timestamp: u64,
}

/// Phase 2: encrypt, sign and post a location update to a paired contact's inbox.
#[tauri::command]
pub async fn send_location_update(
    server_url: String,
    contact_device_id: String,
    latitude: f64,
    longitude: f64,
    accuracy_m: Option<f64>,
    manager: State<'_, PairingManager>,
) -> Result<SentLocationUpdate, String> {
    if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
        return Err("latitude must be a finite number between -90 and 90".to_string());
    }
    if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
        return Err("longitude must be a finite number between -180 and 180".to_string());
    }
    if let Some(accuracy) = accuracy_m {
        if !accuracy.is_finite() || accuracy < 0.0 {
            return Err(
                "accuracy_m must be a finite number greater than or equal to 0".to_string(),
            );
        }
    }

    // All DB work is scoped so the (non-Sync) SQLite handle isn't alive across
    // the network await below.
    let (contact, sequence, payload, package_bytes) = {
        let contacts_db = manager.open_contacts_db()?;
        let contact = contacts_db
            .get_contact(&contact_device_id)
            .map_err(|e| format!("Failed to read contact: {}", e))?
            .ok_or_else(|| {
                format!(
                    "No paired contact with device id {}; pair this device first",
                    contact_device_id
                )
            })?;

        crate::crypt::validate_peer_bundle(&contact.bundle)
            .map_err(|e| format!("{}: {}", contact.device_name, e))?;

        let identity = contacts_db
            .load_identity()
            .map_err(|e| format!("Failed to read local identity: {}", e))?
            .ok_or_else(|| "No local identity on this device; pair this device first".to_string())?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let payload = LocationPayload {
            latitude,
            longitude,
            accuracy_m,
            timestamp,
        };
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to encode location payload: {}", e))?;

        let sequence = manager
            .open_sequence_store()?
            .reserve_next(&identity.bundle.device_id)
            .map_err(|e| format!("Failed to reserve sequence number: {}", e))?;

        let bob_kem_pub = crate::crypt::encapsulation_key_from_bundle(&contact.bundle)?;
        let package = crate::crypt::send_location_update(
            &bob_kem_pub,
            &contact.bundle.x25519_pub,
            &identity.signing_key(),
            identity.bundle.device_id.clone(),
            sequence,
            &payload_bytes,
            timestamp,
        );
        let package_bytes = serde_json::to_vec(&package)
            .map_err(|e| format!("Failed to encode location package: {}", e))?;

        (contact, sequence, payload, package_bytes)
    };

    ServerClient::new(server_url)
        .post_inbox(&contact.device_id, &package_bytes)
        .await?;

    Ok(SentLocationUpdate {
        contact_device_id: contact.device_id,
        contact_device_name: contact.device_name,
        sequence,
        timestamp: payload.timestamp,
    })
}
