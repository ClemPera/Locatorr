//! Tauri command surface. Thin on purpose: all the crypto lives in `locatorr-crypto`, this
//! module's job is identity persistence, contact bookkeeping, and translating between SQLite
//! rows and the JSON DTOs the Svelte frontend talks to over the IPC bridge.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use locatorr_crypto::identity::{Identity, IdentityBytes, PublicBundle};
use locatorr_crypto::pairing;

pub struct AppState {
    pub conn: Mutex<Connection>,
}

#[derive(Serialize)]
pub struct PublicBundleDto {
    pub pairing_payload: String,
}

#[derive(Serialize)]
pub struct ContactDto {
    pub id: String,
    pub nickname: String,
    pub fingerprint: String,
    pub verified: bool,
    pub sharing: bool,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize)]
pub struct SettingsDto {
    pub server_url: String,
    pub poll_interval_secs: i64,
}

#[derive(Serialize)]
pub struct LocationDto {
    pub contact_id: String,
    pub lat: f64,
    pub lon: f64,
    pub accuracy: f64,
    pub updated_at: i64,
}

/// The wire format for a pairing QR/link: just the three public values, base64-JSON. No
/// `user_id` yet, since server registration isn't wired up in this pass — see design doc
/// section 4 for the eventual full shape once that lands.
#[derive(Serialize, Deserialize)]
struct PairingPayload {
    ml_dsa_pub: String,
    kem_pub: String,
    x25519_pub: String,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_or_create_identity(conn: &Connection) -> Result<Identity, String> {
    let existing: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT ml_dsa_priv, kem_decap_priv, x25519_priv FROM identity WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some((ml_dsa_priv, kem_decap_priv, x25519_priv_vec)) = existing {
        let x25519_priv: [u8; 32] = x25519_priv_vec
            .try_into()
            .map_err(|_| "stored x25519_priv is not 32 bytes".to_string())?;
        return Identity::from_bytes(&IdentityBytes {
            ml_dsa_priv,
            kem_decap_priv,
            x25519_priv,
        })
        .map_err(|e| e.to_string());
    }

    let identity = Identity::generate();
    let bytes = identity.to_bytes();
    conn.execute(
        "INSERT INTO identity (id, ml_dsa_priv, kem_decap_priv, x25519_priv) VALUES (1, ?1, ?2, ?3)",
        params![bytes.ml_dsa_priv, bytes.kem_decap_priv, bytes.x25519_priv.to_vec()],
    )
    .map_err(|e| e.to_string())?;
    Ok(identity)
}

fn encode_pairing_payload(bundle: &PublicBundle) -> String {
    let payload = PairingPayload {
        ml_dsa_pub: B64.encode(&bundle.ml_dsa_pub),
        kem_pub: B64.encode(&bundle.kem_pub),
        x25519_pub: B64.encode(bundle.x25519_pub),
    };
    let json = serde_json::to_vec(&payload).expect("PairingPayload always serializes");
    B64.encode(json)
}

fn decode_pairing_payload(payload: &str) -> Result<PublicBundle, String> {
    let json = B64
        .decode(payload.trim())
        .map_err(|_| "pairing payload is not valid base64".to_string())?;
    let parsed: PairingPayload = serde_json::from_slice(&json)
        .map_err(|_| "pairing payload is not valid JSON".to_string())?;

    let ml_dsa_pub = B64
        .decode(parsed.ml_dsa_pub)
        .map_err(|_| "ml_dsa_pub is not valid base64".to_string())?;
    let kem_pub = B64
        .decode(parsed.kem_pub)
        .map_err(|_| "kem_pub is not valid base64".to_string())?;
    let x25519_pub_vec = B64
        .decode(parsed.x25519_pub)
        .map_err(|_| "x25519_pub is not valid base64".to_string())?;
    let x25519_pub: [u8; 32] = x25519_pub_vec
        .try_into()
        .map_err(|_| "x25519_pub is not 32 bytes".to_string())?;

    Ok(PublicBundle {
        ml_dsa_pub,
        kem_pub,
        x25519_pub,
    })
}

fn random_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    B64.encode(bytes)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

#[tauri::command]
pub fn get_pairing_payload(state: tauri::State<AppState>) -> Result<PublicBundleDto, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let identity = load_or_create_identity(&conn)?;
    Ok(PublicBundleDto {
        pairing_payload: encode_pairing_payload(&identity.public_bundle()),
    })
}

#[tauri::command]
pub fn add_contact(
    state: tauri::State<AppState>,
    payload: String,
    nickname: String,
) -> Result<ContactDto, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let my_identity = load_or_create_identity(&conn)?;
    let their_bundle = decode_pairing_payload(&payload)?;

    let fingerprint = pairing::fingerprint(&my_identity.public_bundle(), &their_bundle);
    let id = random_id();
    let created_at = now_unix();

    conn.execute(
        "INSERT INTO contacts (id, nickname, ml_dsa_pub, kem_pub, x25519_pub, fingerprint, verified, sharing, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7)",
        params![
            id,
            nickname,
            their_bundle.ml_dsa_pub,
            their_bundle.kem_pub,
            their_bundle.x25519_pub.to_vec(),
            fingerprint,
            created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(ContactDto {
        id,
        nickname,
        fingerprint,
        verified: false,
        sharing: false,
        created_at,
    })
}

#[tauri::command]
pub fn list_contacts(state: tauri::State<AppState>) -> Result<Vec<ContactDto>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, nickname, fingerprint, verified, sharing, created_at
             FROM contacts ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ContactDto {
                id: row.get(0)?,
                nickname: row.get(1)?,
                fingerprint: row.get(2)?,
                verified: row.get::<_, i64>(3)? != 0,
                sharing: row.get::<_, i64>(4)? != 0,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn verify_contact(state: tauri::State<AppState>, contact_id: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET verified = 1 WHERE id = ?1",
        params![contact_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_contact_sharing(
    state: tauri::State<AppState>,
    contact_id: String,
    sharing: bool,
) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE contacts SET sharing = ?1 WHERE id = ?2",
        params![sharing as i64, contact_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn remove_contact(state: tauri::State<AppState>, contact_id: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM contacts WHERE id = ?1", params![contact_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: tauri::State<AppState>) -> Result<SettingsDto, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let get = |key: &str, default: &str| -> Result<String, String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
        .map(|v: Option<String>| v.unwrap_or_else(|| default.to_string()))
    };
    Ok(SettingsDto {
        server_url: get("server_url", "")?,
        poll_interval_secs: get("poll_interval_secs", "30")?.parse().unwrap_or(30),
    })
}

#[tauri::command]
pub fn update_settings(state: tauri::State<AppState>, settings: SettingsDto) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('server_url', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![settings.server_url],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('poll_interval_secs', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![settings.poll_interval_secs.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Recompute the pairing fingerprint from the contact's stored public keys and the local
/// identity, then compare to the stored fingerprint. Returns `true` if they match (key
/// material hasn't changed since pairing), `false` if the keys have changed (contact may
/// have re-paired/re-installed). The frontend uses this to flag verified contacts whose
/// key material no longer matches the originally-verified fingerprint.
#[tauri::command]
pub fn check_contact_fingerprint(
    state: tauri::State<AppState>,
    contact_id: String,
) -> Result<bool, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let my_identity = load_or_create_identity(&conn)?;

    let (ml_dsa_pub, kem_pub, x25519_pub_vec, stored_fingerprint): (
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        String,
    ) = conn
        .query_row(
            "SELECT ml_dsa_pub, kem_pub, x25519_pub, fingerprint FROM contacts WHERE id = ?1",
            params![contact_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            },
        )
        .map_err(|e| format!("contact not found: {}", e))?;

    let x25519_pub: [u8; 32] = x25519_pub_vec
        .try_into()
        .map_err(|_| "stored x25519_pub is not 32 bytes".to_string())?;

    let contact_bundle = PublicBundle {
        ml_dsa_pub,
        kem_pub,
        x25519_pub,
    };

    let recomputed = pairing::fingerprint(&my_identity.public_bundle(), &contact_bundle);

    Ok(recomputed == stored_fingerprint)
}

/// Always empty in this pass: nothing populates `received_locations` yet, since the relay
/// server HTTP client isn't wired up (see design doc section 10's open questions). The command
/// is real and the table is real, there's just nothing to put in it until that lands.
#[tauri::command]
pub fn list_received_locations(state: tauri::State<AppState>) -> Result<Vec<LocationDto>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT contact_id, lat, lon, accuracy, updated_at FROM received_locations")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(LocationDto {
                contact_id: row.get(0)?,
                lat: row.get(1)?,
                lon: row.get(2)?,
                accuracy: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
