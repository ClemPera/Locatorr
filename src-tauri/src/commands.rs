//! Tauri command surface. Thin on purpose: all the crypto lives in `locatorr-crypto`, this
//! module's job is identity persistence, contact bookkeeping, relay HTTP integration, and
//! translating between SQLite rows and the JSON DTOs the Svelte frontend talks to over the
//! IPC bridge.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri_plugin_store::StoreExt;

use crate::relay;
use locatorr_crypto::envelope;
use locatorr_crypto::identity::{Identity, IdentityBytes, PublicBundle};
use locatorr_crypto::pairing;

pub struct AppState {
    pub conn: Mutex<Connection>,
    pub relay_user_id: Mutex<String>,
}

#[derive(Serialize)]
pub struct PublicBundleDto {
    pub pairing_payload: String,
}

#[derive(Serialize)]
pub struct ContactDto {
    pub id: String,
    pub nickname: String,
    pub relay_user_id: String,
    pub fingerprint: String,
    pub verified: bool,
    pub sharing: bool,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize)]
pub struct SettingsDto {
    pub server_url: String,
    pub poll_interval_secs: i64,
    pub relay_user_id: String,
}

#[derive(Serialize)]
pub struct LocationDto {
    pub contact_id: String,
    pub lat: f64,
    pub lon: f64,
    pub accuracy: f64,
    pub updated_at: i64,
}

/// The wire format for a pairing QR/link. `relay_user_id` is included so the scanning side
/// learns the server-assigned user_id (empty string if the sender hasn't registered yet).
#[derive(Serialize, Deserialize)]
struct PairingPayload {
    ml_dsa_pub: String,
    kem_pub: String,
    x25519_pub: String,
    #[serde(default)]
    relay_user_id: String,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const IDENTITY_STORE: &str = "identity.json";
const KEY_ML_DSA_PRIV: &str = "ml_dsa_priv";
const KEY_KEM_DECAP_PRIV: &str = "kem_decap_priv";
const KEY_X25519_PRIV: &str = "x25519_priv";

fn load_or_create_identity(conn: &Connection, app: &tauri::AppHandle) -> Result<Identity, String> {
    // Try the plugin store first
    if let Ok(store) = app.store(IDENTITY_STORE) {
        let ml_dsa_priv = store
            .get(KEY_ML_DSA_PRIV)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| B64.decode(s).ok());
        let kem_decap_priv = store
            .get(KEY_KEM_DECAP_PRIV)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| B64.decode(s).ok());
        let x25519_priv = store
            .get(KEY_X25519_PRIV)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| B64.decode(s).ok());

        if let (Some(ml_dsa_priv), Some(kem_decap_priv), Some(x25519_priv)) =
            (ml_dsa_priv, kem_decap_priv, x25519_priv)
        {
            let x25519_priv: [u8; 32] = x25519_priv
                .try_into()
                .map_err(|_| "stored x25519_priv is not 32 bytes".to_string())?;
            return Identity::from_bytes(&IdentityBytes {
                ml_dsa_priv,
                kem_decap_priv,
                x25519_priv,
            })
            .map_err(|e| e.to_string());
        }
    }

    // Fallback: load from SQLite (migration path)
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
        let identity = Identity::from_bytes(&IdentityBytes {
            ml_dsa_priv,
            kem_decap_priv,
            x25519_priv,
        })
        .map_err(|e| e.to_string())?;

        // Migrate to plugin store
        let bytes = identity.to_bytes();
        if let Ok(store) = app.store(IDENTITY_STORE) {
            store.set(KEY_ML_DSA_PRIV, json!(B64.encode(&bytes.ml_dsa_priv)));
            store.set(KEY_KEM_DECAP_PRIV, json!(B64.encode(&bytes.kem_decap_priv)));
            store.set(KEY_X25519_PRIV, json!(B64.encode(bytes.x25519_priv)));
            let _ = store.save();
        }
        return Ok(identity);
    }

    // Generate new identity and persist to plugin store
    let identity = Identity::generate();
    let bytes = identity.to_bytes();

    let store = app.store(IDENTITY_STORE).map_err(|e| e.to_string())?;
    store.set(KEY_ML_DSA_PRIV, json!(B64.encode(&bytes.ml_dsa_priv)));
    store.set(KEY_KEM_DECAP_PRIV, json!(B64.encode(&bytes.kem_decap_priv)));
    store.set(KEY_X25519_PRIV, json!(B64.encode(bytes.x25519_priv)));
    store.save().map_err(|e| e.to_string())?;

    // Also store in SQLite for backward compat
    conn.execute(
        "INSERT OR REPLACE INTO identity (id, ml_dsa_priv, kem_decap_priv, x25519_priv) VALUES (1, ?1, ?2, ?3)",
        params![bytes.ml_dsa_priv, bytes.kem_decap_priv, bytes.x25519_priv.to_vec()],
    )
    .map_err(|e| e.to_string())?;

    Ok(identity)
}

fn encode_pairing_payload(bundle: &PublicBundle, relay_user_id: &str) -> String {
    let payload = PairingPayload {
        ml_dsa_pub: B64.encode(&bundle.ml_dsa_pub),
        kem_pub: B64.encode(&bundle.kem_pub),
        x25519_pub: B64.encode(bundle.x25519_pub),
        relay_user_id: relay_user_id.to_string(),
    };
    let json = serde_json::to_vec(&payload).expect("PairingPayload always serializes");
    B64.encode(json)
}

fn decode_pairing_payload(payload: &str) -> Result<(PublicBundle, String), String> {
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

    Ok((
        PublicBundle {
            ml_dsa_pub,
            kem_pub,
            x25519_pub,
        },
        parsed.relay_user_id,
    ))
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

// ── existing commands, updated for new columns ──────────────────────────

#[tauri::command]
pub fn get_pairing_payload(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
) -> Result<PublicBundleDto, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let identity = load_or_create_identity(&conn, &app)?;
    let relay_user_id: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'relay_user_id'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    Ok(PublicBundleDto {
        pairing_payload: encode_pairing_payload(&identity.public_bundle(), &relay_user_id),
    })
}

#[tauri::command]
pub fn add_contact(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    payload: String,
    nickname: String,
) -> Result<ContactDto, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let my_identity = load_or_create_identity(&conn, &app)?;
    let (their_bundle, their_relay_user_id) = decode_pairing_payload(&payload)?;

    let fingerprint = pairing::fingerprint(&my_identity.public_bundle(), &their_bundle);
    let id = random_id();
    let created_at = now_unix();

    conn.execute(
        "INSERT INTO contacts (id, nickname, user_id, ml_dsa_pub, kem_pub, x25519_pub, fingerprint, verified, sharing, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8)",
        params![
            id,
            nickname,
            their_relay_user_id,
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
        relay_user_id: their_relay_user_id,
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
            "SELECT id, nickname, COALESCE(user_id, '') as relay_user_id, fingerprint, verified, sharing, created_at
             FROM contacts ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ContactDto {
                id: row.get(0)?,
                nickname: row.get(1)?,
                relay_user_id: row.get(2)?,
                fingerprint: row.get(3)?,
                verified: row.get::<_, i64>(4)? != 0,
                sharing: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
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
        relay_user_id: get("relay_user_id", "")?,
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
    app: tauri::AppHandle,
    contact_id: String,
) -> Result<bool, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let my_identity = load_or_create_identity(&conn, &app)?;

    let (_user_id, ml_dsa_pub, kem_pub, x25519_pub_vec, stored_fingerprint): (
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        String,
    ) = conn
        .query_row(
            "SELECT COALESCE(user_id, ''), ml_dsa_pub, kem_pub, x25519_pub, fingerprint FROM contacts WHERE id = ?1",
            params![contact_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
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

/// Returns locations received from contacts. Populated by `poll_inbox_for_locations`.
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

// ── relay integration commands ───────────────────────────────────────────

/// Register this device with the relay server. Returns the server-assigned user_id.
/// Idempotent: if already registered, returns the existing user_id without re-registering.
#[tauri::command]
pub async fn register_with_relay(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
) -> Result<String, String> {
    let identity = {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        let identity = load_or_create_identity(&conn, &app)?;

        // Check if already registered
        let existing: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'relay_user_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(ref uid) = existing {
            if !uid.is_empty() {
                return Ok(uid.clone());
            }
        }

        identity
    };
    // Lock is dropped here — safe to await

    let bundle = identity.public_bundle();
    let user_id = relay::register(&server_url, &bundle.ml_dsa_pub, &bundle.kem_pub).await?;

    // Persist the user_id
    {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('relay_user_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![user_id],
        )
        .map_err(|e| e.to_string())?;
    }

    // Update the in-memory cache
    {
        let mut cached = state.relay_user_id.lock().map_err(|e| e.to_string())?;
        *cached = user_id.clone();
    }

    Ok(user_id)
}

/// Authenticate with the relay server: request a challenge nonce, sign it, and exchange it
/// for a session token. The token is stored in settings as "relay_token" for persistence.
/// Returns the token on success.
#[tauri::command]
pub async fn authenticate_with_relay(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
) -> Result<String, String> {
    let (identity, user_id) = {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        let identity = load_or_create_identity(&conn, &app)?;
        let user_id: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'relay_user_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("not registered: call register_with_relay first".to_string())?;
        (identity, user_id)
    };

    let nonce = relay::request_challenge(&server_url, &user_id).await?;
    let signature = identity.sign_challenge(&nonce);
    let (token, _expires_in) = relay::verify_challenge(&server_url, &user_id, &signature).await?;

    {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('relay_token', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![token],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(token)
}

/// Encrypt this device's location and push it to every contact that has sharing enabled.
/// Skips contacts that don't have a relay user_id (not registered with the relay).
#[tauri::command]
pub async fn send_location_update(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
    lat: f64,
    lon: f64,
    accuracy: f64,
) -> Result<(), String> {
    let (identity, token, sharing_contacts) = {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        let identity = load_or_create_identity(&conn, &app)?;

        let token: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'relay_token'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("not authenticated: call authenticate_with_relay first".to_string())?;

        let mut stmt = conn
            .prepare("SELECT user_id, x25519_pub, kem_pub FROM contacts WHERE sharing = 1")
            .map_err(|e| e.to_string())?;
        let contacts: Vec<(String, Vec<u8>, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        (identity, token, contacts)
    };

    let ts = now_unix();
    let payload = serde_json::to_vec(&serde_json::json!({
        "lat": lat,
        "lon": lon,
        "accuracy": accuracy,
        "ts": ts,
    }))
    .map_err(|e| e.to_string())?;

    for (contact_user_id, x25519_pub_vec, kem_pub) in &sharing_contacts {
        if contact_user_id.is_empty() {
            continue; // contact not registered with relay, can't send
        }

        let x25519_pub: [u8; 32] = x25519_pub_vec
            .as_slice()
            .try_into()
            .map_err(|_| "stored x25519_pub is not 32 bytes".to_string())?;

        let share = envelope::share_location(&identity, &x25519_pub, kem_pub, &payload)
            .map_err(|e| e.to_string())?;

        relay::put_location(&server_url, &token, contact_user_id, &share).await?;
    }

    Ok(())
}

/// Poll the relay inbox for new location shares from contacts, decrypt each one, and
/// upsert them into `received_locations`. Returns the list of newly-received locations.
#[tauri::command]
pub async fn poll_inbox_for_locations(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    server_url: String,
) -> Result<Vec<LocationDto>, String> {
    let (identity, token) = {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        let identity = load_or_create_identity(&conn, &app)?;
        let token: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'relay_token'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("not authenticated: call authenticate_with_relay first".to_string())?;
        (identity, token)
    };

    let raw_shares = relay::poll_inbox(&server_url, &token).await?;
    let mut results: Vec<LocationDto> = Vec::new();

    let conn = state.conn.lock().map_err(|e| e.to_string())?;

    for raw in &raw_shares {
        // Look up the contact by relay user_id (the `from` field)
        let contact: Option<(String, Vec<u8>)> = conn
            .query_row(
                "SELECT id, x25519_pub FROM contacts WHERE user_id = ?1",
                params![raw.from],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        let (contact_id, x25519_pub_vec) = match contact {
            Some(c) => c,
            None => continue, // unknown sender, skip
        };

        let x25519_pub: [u8; 32] = x25519_pub_vec
            .as_slice()
            .try_into()
            .map_err(|_| "stored x25519_pub is not 32 bytes".to_string())?;

        let nonce: [u8; 12] = raw
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| "nonce is not 12 bytes".to_string())?;
        let wrap_nonce: [u8; 12] = raw
            .wrap_nonce
            .as_slice()
            .try_into()
            .map_err(|_| "wrap_nonce is not 12 bytes".to_string())?;

        let share = envelope::Share {
            ciphertext: raw.ciphertext.clone(),
            nonce,
            wrapped_key: raw.wrapped_key.clone(),
            wrap_nonce,
            kem_ciphertext: raw.kem_ciphertext.clone(),
        };

        let decrypted = envelope::receive_location(&identity, &x25519_pub, &share)
            .map_err(|e| format!("decrypt failed for contact {}: {}", contact_id, e))?;

        let payload: serde_json::Value = serde_json::from_slice(&decrypted)
            .map_err(|e| format!("bad location payload from contact {}: {}", contact_id, e))?;

        let lat = payload["lat"].as_f64().unwrap_or(0.0);
        let lon = payload["lon"].as_f64().unwrap_or(0.0);
        let accuracy = payload["accuracy"].as_f64().unwrap_or(0.0);
        let ts = payload["ts"].as_i64().unwrap_or(raw.updated_at);

        conn.execute(
            "INSERT INTO received_locations (contact_id, lat, lon, accuracy, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(contact_id) DO UPDATE SET
               lat=excluded.lat, lon=excluded.lon,
               accuracy=excluded.accuracy, updated_at=excluded.updated_at",
            params![contact_id, lat, lon, accuracy, ts],
        )
        .map_err(|e| e.to_string())?;

        results.push(LocationDto {
            contact_id,
            lat,
            lon,
            accuracy,
            updated_at: ts,
        });
    }

    Ok(results)
}
