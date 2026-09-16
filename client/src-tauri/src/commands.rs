use crate::android_location::{AndroidLocation, TrackingEvent};
use crate::crypt::{
    compute_safety_fingerprint, decrypt_from_identity, decrypt_identity_bundle,
    decode_location_package, derive_rendezvous_transit_key, encrypt_identity_bundle,
    generate_rendezvous_invitation, to_hex_groups, ContactStore, LocationPayload, PairedContact,
    ReceivedLocationUpdate, RendezvousInvitation, RendezvousState, ReplayGuard, SequenceStore,
};
use crate::server::ServerClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, Duration};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};

pub struct PairingManager {
    pending: Mutex<HashMap<String, RendezvousState>>,
    established: Mutex<HashMap<String, [u8; 32]>>,
    data_dir: String,
    /// Serializes inbox polling. `GET /inbox/:user_id` deletes on read, so two
    /// concurrent readers can lose a message; the manual poll command and the
    /// background poll loop share this lock. `tokio::sync::Mutex` because it is
    /// held across the HTTP await.
    poll_lock: tokio::sync::Mutex<()>,
}

impl PairingManager {
    pub fn new(data_dir: String) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            established: Mutex::new(HashMap::new()),
            data_dir,
            poll_lock: tokio::sync::Mutex::new(()),
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

    fn replay_db_path(&self) -> String {
        std::path::Path::new(&self.data_dir)
            .join("locatorr_replay.sqlite3")
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

    fn open_replay_guard(&self) -> Result<ReplayGuard, String> {
        ReplayGuard::open(&self.replay_db_path())
            .map_err(|e| format!("Failed to open replay guard DB: {}", e))
    }
}

// ============================================================================
// BACKGROUND TRACKING (Android foreground service + Rust policy)
// ============================================================================

/// Kotlin drives the fix cadence, but a floor keeps a bad caller from draining
/// the battery (and the user's data plan) with a 1 s interval.
pub const MIN_TRACKING_INTERVAL_MS: u64 = 15_000;

/// Policy state for a tracking session. Written only by `start_tracking`; the
/// send cadence itself comes from fixes Kotlin pushes, there is no Rust timer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackingConfig {
    pub contact_device_id: String,
    pub server_url: String,
    pub interval_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackingStatus {
    pub running: bool,
    pub target_device_id: Option<String>,
    pub target_name: Option<String>,
    pub interval_ms: u64,
    pub sent_count: u64,
    pub received_count: u64,
    pub last_error: Option<String>,
    pub last_fix_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PositionFix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: Option<f64>,
    /// Unix seconds (Kotlin reports milliseconds; we normalise at the edge).
    pub timestamp: u64,
}

/// Cap on the UI catch-up buffer. Bounded so a long session cannot grow memory
/// without limit; oldest entries are dropped first.
const MAX_RECEIVED_BUFFER: usize = 500;

#[derive(Default)]
struct TrackingInner {
    config: Option<TrackingConfig>,
    target_name: Option<String>,
    /// Held so a restart is explicit and a second start cannot leak a channel.
    channel: Option<Channel<serde_json::Value>>,
    /// Kept alive so the worker keeps receiving; dropping it (on stop) lets the
    /// worker observe a closed queue.
    sender: Option<mpsc::Sender<TrackingEvent>>,
    /// Closing this stops the worker and the poll loop promptly.
    stop_tx: Option<watch::Sender<bool>>,
    sent_count: u64,
    received_count: u64,
    last_error: Option<String>,
    last_fix_at: Option<u64>,
}

#[derive(Default)]
pub struct TrackingState {
    inner: Mutex<TrackingInner>,
    /// UI catch-up cache: every update this process has decrypted, oldest
    /// first. Deliberately NOT part of the session state, so stopping or
    /// restarting tracking does not clear it. In-memory only; it does not need
    /// to survive process death.
    received: Mutex<Vec<ReceivedLocationUpdate>>,
}

impl TrackingState {
    /// Never panic on a poisoned lock: status reads must not take the app down.
    fn lock(&self) -> std::sync::MutexGuard<'_, TrackingInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn received_lock(&self) -> std::sync::MutexGuard<'_, Vec<ReceivedLocationUpdate>> {
        self.received.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn status(&self) -> TrackingStatus {
        let inner = self.lock();
        TrackingStatus {
            running: inner.config.is_some(),
            target_device_id: inner.config.as_ref().map(|c| c.contact_device_id.clone()),
            target_name: inner.target_name.clone(),
            interval_ms: inner.config.as_ref().map(|c| c.interval_ms).unwrap_or(0),
            sent_count: inner.sent_count,
            received_count: inner.received_count,
            last_error: inner.last_error.clone(),
            last_fix_at: inner.last_fix_at,
        }
    }

    pub fn is_running(&self) -> bool {
        self.lock().config.is_some()
    }

    fn config(&self) -> Option<TrackingConfig> {
        self.lock().config.clone()
    }

    fn begin(
        &self,
        config: TrackingConfig,
        target_name: String,
        channel: Channel<serde_json::Value>,
        sender: mpsc::Sender<TrackingEvent>,
        stop_tx: watch::Sender<bool>,
    ) {
        let mut inner = self.lock();
        inner.config = Some(config);
        inner.target_name = Some(target_name);
        inner.channel = Some(channel);
        inner.sender = Some(sender);
        inner.stop_tx = Some(stop_tx);
        inner.sent_count = 0;
        inner.received_count = 0;
        inner.last_error = None;
        inner.last_fix_at = None;
    }

    fn signal_stop(&self) {
        if let Some(stop_tx) = self.lock().stop_tx.take() {
            let _ = stop_tx.send(true);
        }
    }

    /// Shared body of `clear`/`end_cleanly`. `last_error` is intentionally not
    /// touched, so a user-initiated stop keeps the session's diagnostic.
    fn reset_session(inner: &mut TrackingInner) {
        inner.config = None;
        inner.target_name = None;
        inner.channel = None;
        inner.sender = None;
        inner.stop_tx = None;
        inner.sent_count = 0;
        inner.received_count = 0;
        inner.last_fix_at = None;
    }

    fn clear(&self) {
        Self::reset_session(&mut self.lock());
    }

    /// Ends a session that Kotlin already ended (the user stopped tracking from
    /// the persistent notification, so the service is gone and the activity is
    /// usually gone too). Stops both loops and clears policy state, and drops
    /// any error because a clean stop is not a failure. Never calls Kotlin.
    fn end_cleanly(&self) {
        self.signal_stop();
        let mut inner = self.lock();
        Self::reset_session(&mut inner);
        inner.last_error = None;
    }

    fn record_sent(&self, fix_at: u64) {
        let mut inner = self.lock();
        inner.sent_count += 1;
        inner.last_fix_at = Some(fix_at);
        inner.last_error = None;
    }

    fn record_received(&self, count: u64) {
        let mut inner = self.lock();
        inner.received_count += count;
    }

    /// Appends decrypted updates to the catch-up buffer, dropping the oldest
    /// ones once the cap is reached.
    fn record_received_updates(&self, updates: &[ReceivedLocationUpdate]) {
        if updates.is_empty() {
            return;
        }
        let mut buffer = self.received_lock();
        buffer.extend_from_slice(updates);
        if buffer.len() > MAX_RECEIVED_BUFFER {
            let excess = buffer.len() - MAX_RECEIVED_BUFFER;
            buffer.drain(0..excess);
        }
    }

    /// Snapshot of the catch-up buffer, ascending by sequence so the caller can
    /// treat the last entry as newest (matches what `poll_and_decrypt` returns).
    fn received_updates(&self) -> Vec<ReceivedLocationUpdate> {
        let mut updates = self.received_lock().clone();
        updates.sort_by_key(|update| update.sequence);
        updates
    }

    fn set_error(&self, error: Option<String>) {
        self.lock().last_error = error;
    }
}

fn clamp_tracking_interval(interval_ms: u64) -> u64 {
    interval_ms.max(MIN_TRACKING_INTERVAL_MS)
}

/// Kotlin's timestamps are milliseconds; everything else in the app is seconds.
fn millis_to_unix_seconds(timestamp_ms: i64) -> u64 {
    (timestamp_ms / 1000).max(0) as u64
}

fn emit_status(app: &AppHandle) {
    let status = app.state::<TrackingState>().status();
    if let Err(e) = app.emit("tracking://status", status) {
        eprintln!("[locatorr/android] failed to emit status: {}", e);
    }
}

/// Validates the requested target and returns the policy config plus the
/// contact's display name. Shared by the command and its tests.
fn resolve_tracking_target(
    manager: &PairingManager,
    server_url: &str,
    contact_device_id: &str,
    interval_ms: u64,
) -> Result<(TrackingConfig, String), String> {
    let server_url = server_url.trim();
    if !(server_url.starts_with("http://") || server_url.starts_with("https://")) {
        return Err("serverUrl must start with http:// or https://".to_string());
    }

    let contact_device_id = contact_device_id.trim();
    if contact_device_id.is_empty() {
        return Err("contactDeviceId is required: background tracking needs an explicit target"
            .to_string());
    }

    let contact = manager
        .open_contacts_db()?
        .get_contact(contact_device_id)
        .map_err(|e| format!("Failed to read contact: {}", e))?
        .ok_or_else(|| {
            format!(
                "No paired contact with device id {}; pair this device first",
                contact_device_id
            )
        })?;

    crate::crypt::validate_peer_bundle(&contact.bundle)
        .map_err(|e| format!("{}: {}", contact.device_name, e))?;

    Ok((
        TrackingConfig {
            contact_device_id: contact.device_id.clone(),
            server_url: server_url.to_string(),
            interval_ms: clamp_tracking_interval(interval_ms),
        },
        contact.device_name,
    ))
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

/// Checks a coordinate triple. Shared by the manual send command and the
/// tracking worker so a junk fix from Kotlin is rejected the same way.
fn validate_coordinates(
    latitude: f64,
    longitude: f64,
    accuracy_m: Option<f64>,
) -> Result<(), String> {
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
    Ok(())
}

/// Encrypts, signs and posts a location update to a paired contact's inbox.
/// Shared by the `send_location_update` command and the tracking worker.
async fn send_update_to_contact(
    manager: &PairingManager,
    server_url: &str,
    contact_device_id: &str,
    latitude: f64,
    longitude: f64,
    accuracy_m: Option<f64>,
) -> Result<SentLocationUpdate, String> {
    validate_coordinates(latitude, longitude, accuracy_m)?;

    // All DB work is scoped so the (non-Sync) SQLite handle isn't alive across
    // the network await below.
    let (contact, sequence, payload, package_bytes) = {
        let contacts_db = manager.open_contacts_db()?;
        let contact = contacts_db
            .get_contact(contact_device_id)
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

    ServerClient::new(server_url.to_string())
        .post_inbox(&contact.device_id, &package_bytes)
        .await?;

    Ok(SentLocationUpdate {
        contact_device_id: contact.device_id,
        contact_device_name: contact.device_name,
        sequence,
        timestamp: payload.timestamp,
    })
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
    send_update_to_contact(
        &manager,
        &server_url,
        &contact_device_id,
        latitude,
        longitude,
        accuracy_m,
    )
    .await
}

/// Phase 3: fetch and decrypt every location update queued for this device.
#[tauri::command]
pub async fn poll_location_updates(
    app: AppHandle,
    server_url: String,
) -> Result<Vec<ReceivedLocationUpdate>, String> {
    poll_and_decrypt(&app, &server_url).await
}

/// Everything this process has decrypted, oldest first (ascending by sequence).
/// A UI that was not mounted when `tracking://received` fired can catch up here.
#[tauri::command]
pub fn get_received_updates(app: AppHandle) -> Result<Vec<ReceivedLocationUpdate>, String> {
    Ok(app.state::<TrackingState>().received_updates())
}

/// One inbox read. `GET /inbox/:user_id` deletes on read, so this takes the
/// shared poll lock: a second concurrent poller simply sees an empty inbox
/// rather than racing the first one and losing a message.
async fn poll_and_decrypt(
    app: &AppHandle,
    server_url: &str,
) -> Result<Vec<ReceivedLocationUpdate>, String> {
    let manager = app.state::<PairingManager>();
    let _guard = manager.poll_lock.lock().await;

    let identity = manager
        .open_contacts_db()?
        .load_identity()
        .map_err(|e| format!("Failed to read local identity: {}", e))?
        .ok_or_else(|| "No local identity on this device; pair this device first".to_string())?;

    // The server deletes messages as it returns them, so this is the only copy
    // we will ever see of these updates.
    let messages = ServerClient::new(server_url.to_string())
        .get_inbox(&identity.bundle.device_id)
        .await?;

    // Everything below is synchronous SQLite work, kept out of the async part
    // so the non-Sync connection is never held across an await.
    let contacts_db = manager.open_contacts_db()?;
    let replay_guard = manager.open_replay_guard()?;

    let mut received = Vec::new();
    for message in messages {
        let package = match decode_location_package(&message.payload) {
            Ok(package) => package,
            Err(e) => {
                eprintln!("[locatorr/android] skipping inbox message {}: {}", message.id, e);
                continue;
            }
        };

        // Looking the contact up by the package's *claimed* sender_id is safe:
        // the ML-DSA signature covers sender_id and is verified against the
        // pinned key of whichever contact that id resolves to, so a forged
        // sender_id cannot authenticate and is dropped below.
        let contact = match contacts_db.get_contact(&package.sender_id) {
            Ok(Some(contact)) => contact,
            Ok(None) => {
                eprintln!(
                    "[locatorr/android] skipping inbox message {}: unknown sender {}",
                    message.id, package.sender_id
                );
                continue;
            }
            Err(e) => {
                eprintln!(
                    "[locatorr/android] skipping inbox message {}: contact lookup failed: {}",
                    message.id, e
                );
                continue;
            }
        };

        match decrypt_from_identity(&package, &identity, &contact.bundle, &replay_guard) {
            Ok(payload) => received.push(ReceivedLocationUpdate {
                sender_id: contact.device_id,
                sender_name: contact.device_name,
                sequence: package.sequence,
                timestamp: payload.timestamp,
                latitude: payload.latitude,
                longitude: payload.longitude,
                accuracy_m: payload.accuracy_m,
            }),
            Err(e) => {
                eprintln!(
                    "[locatorr/android] skipping inbox message {} from {}: {}",
                    message.id, contact.device_id, e
                );
            }
        }
    }

    // Ascending by sequence so the caller can treat the last entry as newest.
    received.sort_by_key(|update| update.sequence);

    // Both the manual command and the background loop decrypt through here, so
    // this buffer is the single record of what this process has received.
    app.state::<TrackingState>().record_received_updates(&received);

    Ok(received)
}

// ============================================================================
// BACKGROUND TRACKING COMMANDS
// ============================================================================

/// Starts Android background tracking. Refuses without an explicit, valid
/// target: even a single paired contact is not assumed to be the destination.
#[tauri::command]
pub async fn start_tracking(
    app: AppHandle,
    server_url: String,
    contact_device_id: String,
    interval_ms: u64,
) -> Result<(), String> {
    let (config, target_name) = {
        let manager = app.state::<PairingManager>();
        resolve_tracking_target(&manager, &server_url, &contact_device_id, interval_ms)?
    };

    // Restarting requires an explicit stop; a second start must not leak a
    // channel id or leave two workers running.
    if app.state::<TrackingState>().is_running() {
        eprintln!("[locatorr/android] tracking already running; ignoring start_tracking");
        return Ok(());
    }

    let (sender, fixes) = mpsc::channel::<TrackingEvent>(64);
    let channel = Channel::<serde_json::Value>::new({
        let sender = sender.clone();
        // This runs on the Kotlin LocationListener thread: hand the event to
        // the worker and return. No blocking, no crypto, no network, no unwrap.
        move |body| {
            match body.deserialize::<TrackingEvent>() {
                Ok(event) => {
                    if sender.try_send(event).is_err() {
                        eprintln!(
                            "[locatorr/android] tracking event dropped: worker queue full or closed"
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[locatorr/android] ignoring malformed tracking event: {}", e);
                }
            }
            Ok(())
        }
    });

    let (stop_tx, stop_rx) = watch::channel(false);

    app.state::<TrackingState>().begin(
        config.clone(),
        target_name.clone(),
        channel.clone(),
        sender,
        stop_tx,
    );

    // Kotlin owns the service. The fix channel queues into the worker, so
    // nothing is lost between here and the spawns below. The target name goes
    // into the always-shown notification.
    let location = app.state::<AndroidLocation<tauri::Wry>>();
    if let Err(e) = location
        .start(config.interval_ms, &target_name, channel)
        .await
    {
        eprintln!("[locatorr/android] failed to start tracking: {}", e);
        app.state::<TrackingState>().signal_stop();
        app.state::<TrackingState>().clear();
        emit_status(&app);
        return Err(e);
    }

    spawn_tracking_worker(app.clone(), fixes, stop_rx.clone());
    spawn_poll_loop(app.clone(), config.interval_ms, stop_rx);

    emit_status(&app);
    Ok(())
}

/// Stops background tracking. Idempotent: stopping while idle is a no-op.
#[tauri::command]
pub async fn stop_tracking(app: AppHandle) -> Result<(), String> {
    stop_tracking_inner(&app).await
}

/// One-shot position, used to send a single manual update.
#[tauri::command]
pub async fn get_current_position(app: AppHandle) -> Result<PositionFix, String> {
    let native = app
        .state::<AndroidLocation<tauri::Wry>>()
        .get_current_position()
        .await?;

    Ok(PositionFix {
        latitude: native.latitude,
        longitude: native.longitude,
        accuracy_m: if native.accuracy >= 0.0 {
            Some(native.accuracy as f64)
        } else {
            None
        },
        timestamp: millis_to_unix_seconds(native.timestamp),
    })
}

async fn stop_tracking_inner(app: &AppHandle) -> Result<(), String> {
    let location = app.state::<AndroidLocation<tauri::Wry>>();
    let was_running = app.state::<TrackingState>().is_running() || location.is_active();
    if !was_running {
        return Ok(());
    }

    // 1. Tell the worker and the poll loop to exit.
    app.state::<TrackingState>().signal_stop();

    // 2. Kotlin owns the service and the notification, so it must be the one to
    //    tear them down (guarded: the activity may already be gone).
    let result = location.stop().await;

    // 3. Clear policy state regardless, so a later start is not blocked by a
    //    stale session.
    app.state::<TrackingState>().clear();
    emit_status(app);
    result
}

/// Handles one fix: re-validate the target, then send. A single failure is
/// recorded and the loop keeps going.
async fn handle_fix(
    app: &AppHandle,
    config: &TrackingConfig,
    latitude: f64,
    longitude: f64,
    accuracy: f32,
    timestamp_ms: i64,
) {
    let manager = app.state::<PairingManager>();

    // The target can be unpaired at any time while we are tracking, so it is
    // re-validated on every fix rather than once at start.
    let target_still_paired = match manager.open_contacts_db() {
        Ok(db) => match db.get_contact(&config.contact_device_id) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                eprintln!("[locatorr/android] contact lookup failed: {}", e);
                app.state::<TrackingState>().set_error(Some(e.to_string()));
                emit_status(app);
                return;
            }
        },
        Err(e) => {
            eprintln!("[locatorr/android] {}", e);
            app.state::<TrackingState>().set_error(Some(e));
            emit_status(app);
            return;
        }
    };

    if !target_still_paired {
        eprintln!(
            "[locatorr/android] target {} is no longer paired; stopping tracking",
            config.contact_device_id
        );
        app.state::<TrackingState>()
            .set_error(Some("target contact was removed; tracking stopped".to_string()));
        let _ = stop_tracking_inner(app).await;
        return;
    }

    let accuracy_m = if accuracy >= 0.0 {
        Some(accuracy as f64)
    } else {
        None
    };

    match send_update_to_contact(
        &manager,
        &config.server_url,
        &config.contact_device_id,
        latitude,
        longitude,
        accuracy_m,
    )
    .await
    {
        Ok(_) => app
            .state::<TrackingState>()
            .record_sent(millis_to_unix_seconds(timestamp_ms)),
        Err(e) => {
            eprintln!("[locatorr/android] send failed: {}", e);
            app.state::<TrackingState>().set_error(Some(e));
        }
    }

    emit_status(app);
}

/// How often an idle worker re-checks the stop flag. The mobile channel
/// registry keeps a clone of the `Channel` (and therefore of our mpsc sender)
/// alive for the process lifetime, so "the queue closed" is not a reliable stop
/// signal on Android.
const WORKER_STOP_POLL: Duration = Duration::from_secs(1);

fn spawn_tracking_worker(
    app: AppHandle,
    mut fixes: mpsc::Receiver<TrackingEvent>,
    stop: watch::Receiver<bool>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match tokio::time::timeout(WORKER_STOP_POLL, fixes.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(_) => {
                    if *stop.borrow() {
                        break;
                    }
                    continue;
                }
            };

            let Some(config) = app.state::<TrackingState>().config() else {
                break;
            };

            match event {
                TrackingEvent::Fix {
                    latitude,
                    longitude,
                    accuracy,
                    timestamp,
                    provider: _,
                } => handle_fix(&app, &config, latitude, longitude, accuracy, timestamp).await,
                TrackingEvent::Error { message } => {
                    eprintln!("[locatorr/android] location plugin error: {}", message);
                    app.state::<TrackingState>().set_error(Some(message));
                    emit_status(&app);
                }
                TrackingEvent::PermissionDenied { message } => {
                    eprintln!("[locatorr/android] location permission denied: {}", message);
                    app.state::<TrackingState>().set_error(Some(message));
                    let _ = stop_tracking_inner(&app).await;
                }
                TrackingEvent::Stopped { reason } => {
                    eprintln!("[locatorr/android] location service stopped: {}", reason);
                    // Kotlin already stopped the service and detached the
                    // channel, so there is nothing to call back into (and the
                    // activity is usually gone, which would trip the no-window
                    // guard). End the session Rust-side only: `running` goes
                    // false, which also releases `prevent_exit`, and no error is
                    // surfaced -- a user stopping from the notification is not a
                    // failure.
                    app.state::<TrackingState>().end_cleanly();
                    emit_status(&app);
                }
            }
        }
    });
}

fn spawn_poll_loop(app: AppHandle, interval_ms: u64, mut stop: watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let interval = Duration::from_millis(interval_ms);
        let mut next_tick = tokio::time::Instant::now();

        loop {
            // Wait for the next tick or a stop signal, whichever comes first.
            if tokio::time::timeout_at(next_tick, stop.changed()).await.is_ok() {
                break;
            }

            match app.state::<TrackingState>().config() {
                Some(config) => {
                    match poll_and_decrypt(&app, &config.server_url).await {
                        Ok(updates) if !updates.is_empty() => {
                            app.state::<TrackingState>().record_received(updates.len() as u64);
                            if let Err(e) = app.emit("tracking://received", updates) {
                                eprintln!(
                                    "[locatorr/android] failed to emit received updates: {}",
                                    e
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("[locatorr/android] inbox poll failed: {}", e);
                            app.state::<TrackingState>().set_error(Some(e));
                        }
                    }
                    emit_status(&app);
                }
                // Config was cleared: the session is over.
                None => break,
            }

            // Schedule from "now" rather than the previous deadline so a slow
            // poll cannot make the loop spin catching up on missed ticks.
            next_tick = tokio::time::Instant::now() + interval;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypt::LocalIdentity;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locatorr_track_{}_{}",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn manager_with_contact(label: &str, device_id: &str) -> PairingManager {
        let manager = PairingManager::new(temp_dir(label).to_string_lossy().to_string());
        let peer = LocalIdentity::generate("peer");
        let contact = PairedContact {
            device_id: device_id.to_string(),
            device_name: "peer".to_string(),
            fingerprint: "ab cd ef".to_string(),
            bundle: peer.bundle,
            paired_at: 0,
        };
        manager
            .open_contacts_db()
            .expect("open contacts db")
            .save_contact(&contact)
            .expect("save contact");
        manager
    }

    #[test]
    fn tracking_interval_is_clamped_to_floor() {
        assert_eq!(clamp_tracking_interval(0), MIN_TRACKING_INTERVAL_MS);
        assert_eq!(clamp_tracking_interval(1_000), MIN_TRACKING_INTERVAL_MS);
        assert_eq!(clamp_tracking_interval(60_000), 60_000);
    }

    #[test]
    fn start_tracking_rejects_missing_target() {
        let manager = manager_with_contact("missing_target", "abc123");

        let err = resolve_tracking_target(&manager, "http://localhost:8080", "   ", 15_000)
            .expect_err("empty target must be rejected");
        assert!(err.contains("contactDeviceId"), "unexpected error: {}", err);
    }

    #[test]
    fn start_tracking_rejects_unknown_contact() {
        let manager = manager_with_contact("unknown_contact", "abc123");

        let err = resolve_tracking_target(&manager, "http://localhost:8080", "nope", 15_000)
            .expect_err("unknown contact must be rejected");
        assert!(err.contains("No paired contact"), "unexpected error: {}", err);
    }

    #[test]
    fn start_tracking_rejects_bad_server_url() {
        let manager = manager_with_contact("bad_url", "abc123");

        let err = resolve_tracking_target(&manager, "localhost:8080", "abc123", 15_000)
            .expect_err("scheme-less URL must be rejected");
        assert!(err.contains("serverUrl"), "unexpected error: {}", err);
    }

    #[test]
    fn resolve_tracking_target_trims_url_and_clamps_interval() {
        let manager = manager_with_contact("resolve_ok", "abc123");

        let (config, name) =
            resolve_tracking_target(&manager, "  http://localhost:8080  ", "abc123", 5_000)
                .expect("valid target must resolve");

        assert_eq!(config.contact_device_id, "abc123");
        assert_eq!(config.server_url, "http://localhost:8080");
        assert_eq!(config.interval_ms, MIN_TRACKING_INTERVAL_MS);
        assert_eq!(name, "peer");
    }

    /// The inbox deletes on read, so two pollers must never be inside at once.
    #[test]
    fn poll_lock_serializes_concurrent_pollers() {
        let manager = Arc::new(PairingManager::new(
            temp_dir("poll_lock").to_string_lossy().to_string(),
        ));
        let inside = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let mut tasks = Vec::new();
        for _ in 0..2 {
            let manager = Arc::clone(&manager);
            let inside = Arc::clone(&inside);
            let peak = Arc::clone(&peak);
            let barrier = Arc::clone(&barrier);

            tasks.push(tauri::async_runtime::spawn(async move {
                // Both tasks start contending at the same moment.
                barrier.wait().await;

                let _guard = manager.poll_lock.lock().await;
                let concurrent = inside.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(concurrent, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                inside.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        tauri::async_runtime::block_on(async {
            for task in tasks {
                task.await.expect("poll task panicked");
            }
        });

        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "the poll lock must serialize concurrent pollers"
        );
    }

    fn sample_update(sequence: u64) -> ReceivedLocationUpdate {
        ReceivedLocationUpdate {
            sender_id: "peer".to_string(),
            sender_name: "peer".to_string(),
            sequence,
            timestamp: sequence,
            latitude: 0.0,
            longitude: 0.0,
            accuracy_m: None,
        }
    }

    /// Builds a running session without an `AppHandle`, so state transitions can
    /// be exercised directly.
    fn started_state() -> TrackingState {
        let state = TrackingState::default();
        let config = TrackingConfig {
            contact_device_id: "abc123".to_string(),
            server_url: "http://localhost:8080".to_string(),
            interval_ms: MIN_TRACKING_INTERVAL_MS,
        };
        let (stop_tx, _stop_rx) = watch::channel(false);
        let (sender, _fixes) = mpsc::channel(1);
        let channel = Channel::<serde_json::Value>::new(|_| Ok(()));
        state.begin(config, "peer".to_string(), channel, sender, stop_tx);
        state
    }

    /// The `Stopped` event path calls `end_cleanly`; it must not surface the
    /// reason as an error (a notification stop is not a failure).
    #[test]
    fn clean_stop_clears_running_without_an_error() {
        let state = started_state();
        state.set_error(Some("stale transient error".to_string()));
        assert!(state.is_running());

        state.end_cleanly();

        let status = state.status();
        assert!(!status.running);
        assert_eq!(status.last_error, None);
        assert_eq!(status.target_device_id, None);
        assert_eq!(status.target_name, None);
        assert_eq!(status.sent_count, 0);
    }

    #[test]
    fn received_buffer_drops_oldest_at_the_cap() {
        let state = TrackingState::default();

        let initial: Vec<_> = (0..MAX_RECEIVED_BUFFER as u64).map(sample_update).collect();
        state.record_received_updates(&initial);
        assert_eq!(state.received_updates().len(), MAX_RECEIVED_BUFFER);

        state.record_received_updates(&[sample_update(9_999)]);
        let buffer = state.received_updates();
        assert_eq!(buffer.len(), MAX_RECEIVED_BUFFER);
        assert_eq!(buffer.first().expect("non-empty").sequence, 1);
        assert_eq!(buffer.last().expect("non-empty").sequence, 9_999);
    }

    /// The buffer is a UI catch-up cache for the whole process, not session
    /// state, so stopping tracking must not drop it.
    #[test]
    fn stopping_tracking_keeps_the_received_buffer() {
        let state = started_state();
        state.record_received_updates(&[sample_update(1), sample_update(2)]);

        state.clear();
        assert!(!state.is_running());
        assert_eq!(state.received_updates().len(), 2);

        state.end_cleanly();
        assert_eq!(state.received_updates().len(), 2);
    }
}
