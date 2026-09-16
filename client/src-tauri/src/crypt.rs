use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce as AesNonce,
};
use serde::{Serialize, Deserialize};
use core::convert::TryFrom;
use hkdf::Hkdf;
use ml_dsa::{Keypair, MlDsa65, Signer, SignatureEncoding, Verifier};
use ml_kem::ml_kem_768::{Ciphertext as KemCiphertext, DecapsulationKey, EncapsulationKey};
// `Generate` (from crypto-common) is the trait that provides `generate_from_rng`
// for both ML-KEM and ML-DSA keys, so a single import serves both.
use ml_kem::{Decapsulate, Encapsulate, Generate, KeyExport};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret};

// ============================================================================
// PHASE 1: PAIRING & BOOTSTRAPPING (unchanged from the original design)
// ============================================================================

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RendezvousInvitation {
    pub rendezvous_id: String,
    pub x_temp_pub: [u8; 32],
    pub token: [u8; 32],
}

pub struct RendezvousState {
    pub x_temp_sec: EphemeralSecret,
    pub token: [u8; 32],
}

//TODO: This should be server side probably
pub fn generate_rendezvous_invitation(rendezvous_id: String) -> (RendezvousState, RendezvousInvitation) {
    let mut rng = rand::rng();
    let mut token = [0u8; 32];
    rand::RngExt::fill(&mut rng, &mut token);

    let x_temp_sec = EphemeralSecret::random_from_rng(&mut rng);
    let x_temp_pub = X25519PublicKey::from(&x_temp_sec);

    let invitation = RendezvousInvitation {
        rendezvous_id,
        x_temp_pub: *x_temp_pub.as_bytes(),
        token,
    };
    let state = RendezvousState { x_temp_sec, token };
    (state, invitation)
}

pub fn derive_rendezvous_transit_key(
    my_secret: EphemeralSecret,
    their_pub_bytes: &[u8; 32],
    token: &[u8; 32],
) -> [u8; 32] {
    let their_pub = X25519PublicKey::from(*their_pub_bytes);
    let dh_secret = my_secret.diffie_hellman(&their_pub);

    let hk = Hkdf::<Sha256>::new(Some(token), dh_secret.as_bytes());
    let mut transit_key = [0u8; 32];
    hk.expand(b"rendezvous_channel", &mut transit_key)
        .expect("HKDF expand failed");
    transit_key
}

pub fn compute_safety_fingerprint(bundle_a: &[u8], bundle_b: &[u8]) -> [u8; 12] {
    let mut hasher = Sha256::new();
    if bundle_a < bundle_b {
        hasher.update(bundle_a);
        hasher.update(bundle_b);
    } else {
        hasher.update(bundle_b);
        hasher.update(bundle_a);
    }
    let hash = hasher.finalize();
    let mut fingerprint = [0u8; 12];
    fingerprint.copy_from_slice(&hash[0..12]);
    fingerprint
}

// ============================================================================
// IDENTITY & ENCRYPTED IDENTITY BUNDLE EXCHANGE
// ============================================================================

// Serialized sizes of the post-quantum key material. Kept here so the exchange
// code can reject truncated bundles instead of failing later with a cryptic
// parse error.
pub const ML_KEM_768_PUBLIC_KEY_LEN: usize = 1184;
pub const ML_KEM_768_SEED_LEN: usize = 64;
pub const ML_DSA_65_PUBLIC_KEY_LEN: usize = 1952;
pub const ML_DSA_65_SEED_LEN: usize = 32;

pub fn to_hex_groups(bytes: &[u8]) -> String {
    bytes
        .chunks(2)
        .map(|pair| pair.iter().map(|b| format!("{:02x}", b)).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// First 8 bytes of the X25519 public key, hex-encoded without separators.
/// Stable for the lifetime of the device because the private key is persisted.
fn derive_device_id(x25519_pub: &[u8; 32]) -> String {
    to_hex_groups(&x25519_pub[0..8]).replace(' ', "")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IdentityBundle {
    pub device_id: String,
    pub device_name: String,
    pub x25519_pub: [u8; 32],
    pub ml_kem_pub: Vec<u8>,
    pub ml_dsa_pub: Vec<u8>,
}

/// This device's static identity: generated once and reused for every pairing
/// and every location update. Only the private material is persisted; the
/// public bundle is derived from it on load.
#[derive(Clone, Debug)]
pub struct LocalIdentity {
    pub bundle: IdentityBundle,
    pub x25519_private: [u8; 32],
    pub ml_kem_seed: [u8; 64],
    pub ml_dsa_seed: [u8; 32],
}

impl LocalIdentity {
    pub fn generate(device_name: &str) -> Self {
        let mut rng = rand::rng();
        let x25519_secret = StaticSecret::random_from_rng(&mut rng);
        let ml_kem_dk: DecapsulationKey = Generate::generate_from_rng(&mut rng);
        let ml_dsa_sk = ml_dsa::SigningKey::<MlDsa65>::generate_from_rng(&mut rng);

        let x25519_pub = X25519PublicKey::from(&x25519_secret);
        let ml_kem_seed: [u8; ML_KEM_768_SEED_LEN] = ml_kem_dk
            .to_seed()
            .expect("ML-KEM-768 decapsulation key is seed-representable")
            .into();
        let ml_dsa_seed: [u8; ML_DSA_65_SEED_LEN] = ml_dsa_sk.to_seed().into();

        let bundle = IdentityBundle {
            device_id: derive_device_id(x25519_pub.as_bytes()),
            device_name: device_name.to_string(),
            x25519_pub: *x25519_pub.as_bytes(),
            ml_kem_pub: KeyExport::to_bytes(ml_kem_dk.encapsulation_key())
                .as_slice()
                .to_vec(),
            ml_dsa_pub: KeyExport::to_bytes(&ml_dsa_sk.verifying_key())
                .as_slice()
                .to_vec(),
        };

        Self {
            bundle,
            x25519_private: x25519_secret.to_bytes(),
            ml_kem_seed,
            ml_dsa_seed,
        }
    }

    /// Rebuilds the identity (including all public keys) from persisted private
    /// material. The `device_id` is stored rather than re-derived so that it
    /// stays stable even if the derivation ever changes.
    pub fn from_persisted(
        device_name: &str,
        device_id: &str,
        x25519_private: [u8; 32],
        ml_kem_seed: [u8; 64],
        ml_dsa_seed: [u8; 32],
    ) -> Result<Self, String> {
        let x25519_secret = StaticSecret::from(x25519_private);
        let x25519_pub = X25519PublicKey::from(&x25519_secret);

        let kem_seed = ml_kem::Seed::try_from(&ml_kem_seed[..])
            .map_err(|_| "invalid ML-KEM seed length".to_string())?;
        let kem_dk = DecapsulationKey::from_seed(kem_seed);

        let dsa_seed = ml_dsa::Seed::try_from(&ml_dsa_seed[..])
            .map_err(|_| "invalid ML-DSA seed length".to_string())?;
        let dsa_sk = ml_dsa::SigningKey::<MlDsa65>::from_seed(&dsa_seed);

        let bundle = IdentityBundle {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            x25519_pub: *x25519_pub.as_bytes(),
            ml_kem_pub: KeyExport::to_bytes(kem_dk.encapsulation_key())
                .as_slice()
                .to_vec(),
            ml_dsa_pub: KeyExport::to_bytes(&dsa_sk.verifying_key())
                .as_slice()
                .to_vec(),
        };

        Ok(Self {
            bundle,
            x25519_private,
            ml_kem_seed,
            ml_dsa_seed,
        })
    }

    pub fn signing_key(&self) -> ml_dsa::SigningKey<MlDsa65> {
        // Seeds are fixed-size arrays, so the conversion cannot fail.
        let seed = ml_dsa::Seed::from(self.ml_dsa_seed);
        ml_dsa::SigningKey::from_seed(&seed)
    }

    pub fn kem_decapsulation_key(&self) -> Result<DecapsulationKey, String> {
        let seed = ml_kem::Seed::try_from(&self.ml_kem_seed[..])
            .map_err(|_| "invalid ML-KEM seed length".to_string())?;
        Ok(DecapsulationKey::from_seed(seed))
    }

    pub fn x25519_secret(&self) -> StaticSecret {
        StaticSecret::from(self.x25519_private)
    }
}

pub fn encapsulation_key_from_bundle(bundle: &IdentityBundle) -> Result<EncapsulationKey, String> {
    let key = ml_kem::Key::<EncapsulationKey>::try_from(bundle.ml_kem_pub.as_slice())
        .map_err(|_| "invalid ML-KEM public key length".to_string())?;
    EncapsulationKey::new(&key).map_err(|_| "invalid ML-KEM public key".to_string())
}

pub fn verifying_key_from_bundle(
    bundle: &IdentityBundle,
) -> Result<ml_dsa::VerifyingKey<MlDsa65>, String> {
    let key = ml_dsa::EncodedVerifyingKey::<MlDsa65>::try_from(bundle.ml_dsa_pub.as_slice())
        .map_err(|_| "invalid ML-DSA public key length".to_string())?;
    Ok(ml_dsa::VerifyingKey::<MlDsa65>::new(&key))
}

pub fn validate_peer_bundle(bundle: &IdentityBundle) -> Result<(), String> {
    if bundle.ml_kem_pub.len() != ML_KEM_768_PUBLIC_KEY_LEN
        || bundle.ml_dsa_pub.len() != ML_DSA_65_PUBLIC_KEY_LEN
    {
        return Err(
            "peer identity bundle has no post-quantum keys; re-pair this device with the current build"
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PairedContact {
    pub device_id: String,
    pub device_name: String,
    pub fingerprint: String,
    pub bundle: IdentityBundle,
    pub paired_at: u64,
}

pub fn encrypt_identity_bundle(
    transit_key: &[u8; 32],
    bundle: &IdentityBundle,
) -> Result<Vec<u8>, String> {
    let json_bytes = serde_json::to_vec(bundle).map_err(|e| format!("Serialization error: {}", e))?;
    let mut rng = rand::rng();
    let mut nonce_bytes = [0u8; 12];
    rand::RngExt::fill(&mut rng, &mut nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(transit_key).map_err(|e| format!("Invalid transit key: {}", e))?;
    let nonce = AesNonce::try_from(&nonce_bytes[..]).map_err(|e| format!("Invalid nonce: {}", e))?;

    let ciphertext = cipher
        .encrypt(&nonce, json_bytes.as_slice())
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt_identity_bundle(
    transit_key: &[u8; 32],
    encrypted_data: &[u8],
) -> Result<IdentityBundle, String> {
    if encrypted_data.len() < 13 {
        return Err("Encrypted bundle payload too short".to_string());
    }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(transit_key).map_err(|e| format!("Invalid transit key: {}", e))?;
    let nonce = AesNonce::try_from(nonce_bytes).map_err(|e| format!("Invalid nonce: {}", e))?;

    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {}", e))?;

    serde_json::from_slice(&plaintext).map_err(|e| format!("Deserialization error: {}", e))
}

pub struct ContactStore {
    conn: Connection,
}

/// Maps the fixed column order used by every `paired_contacts` query.
fn row_to_contact(row: &rusqlite::Row) -> rusqlite::Result<PairedContact> {
    let device_id: String = row.get(0)?;
    let device_name: String = row.get(1)?;
    let fingerprint: String = row.get(2)?;
    let bundle_json: String = row.get(3)?;
    let paired_at: i64 = row.get(4)?;
    let bundle: IdentityBundle = serde_json::from_str(&bundle_json).unwrap_or_else(|_| IdentityBundle {
        device_id: device_id.clone(),
        device_name: device_name.clone(),
        x25519_pub: [0u8; 32],
        ml_kem_pub: vec![],
        ml_dsa_pub: vec![],
    });
    Ok(PairedContact {
        device_id,
        device_name,
        fingerprint,
        bundle,
        paired_at: paired_at as u64,
    })
}

fn blob_to_array<const N: usize>(bytes: &[u8]) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            format!("expected {} bytes, got {}", N, bytes.len()).into(),
        )
    })
}

impl ContactStore {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS paired_contacts (
                device_id   TEXT PRIMARY KEY,
                device_name TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                bundle_json TEXT NOT NULL,
                paired_at   INTEGER NOT NULL
            )",
            [],
        )?;
        // NOTE: the private keys below are stored unencrypted in the local
        // SQLite file. That is fine while the app owns the file, but production
        // should move this row into the OS keychain.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS local_identity (
                id           INTEGER PRIMARY KEY CHECK (id = 1),
                device_id    TEXT NOT NULL,
                device_name  TEXT NOT NULL,
                x25519_priv  BLOB NOT NULL,
                ml_kem_seed  BLOB NOT NULL,
                ml_dsa_seed  BLOB NOT NULL,
                created_at   INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn save_contact(&self, contact: &PairedContact) -> rusqlite::Result<()> {
        let bundle_json = serde_json::to_string(&contact.bundle).unwrap_or_default();
        self.conn.execute(
            "INSERT INTO paired_contacts (device_id, device_name, fingerprint, bundle_json, paired_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(device_id) DO UPDATE SET
                device_name = excluded.device_name,
                fingerprint = excluded.fingerprint,
                bundle_json = excluded.bundle_json,
                paired_at = excluded.paired_at",
            params![
                contact.device_id,
                contact.device_name,
                contact.fingerprint,
                bundle_json,
                contact.paired_at as i64
            ],
        )?;
        Ok(())
    }

    pub fn get_contacts(&self) -> rusqlite::Result<Vec<PairedContact>> {
        let mut stmt = self.conn.prepare(
            "SELECT device_id, device_name, fingerprint, bundle_json, paired_at FROM paired_contacts ORDER BY paired_at DESC"
        )?;
        let rows = stmt.query_map([], row_to_contact)?;

        let mut contacts = Vec::new();
        for r in rows {
            contacts.push(r?);
        }
        Ok(contacts)
    }

    pub fn get_contact(&self, device_id: &str) -> rusqlite::Result<Option<PairedContact>> {
        self.conn
            .query_row(
                "SELECT device_id, device_name, fingerprint, bundle_json, paired_at
                 FROM paired_contacts WHERE device_id = ?1",
                params![device_id],
                row_to_contact,
            )
            .optional()
    }

    pub fn delete_contact(&self, device_id: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM paired_contacts WHERE device_id = ?1", params![device_id])?;
        Ok(())
    }

    pub fn load_identity(&self) -> rusqlite::Result<Option<LocalIdentity>> {
        let row: Option<(String, String, Vec<u8>, Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT device_id, device_name, x25519_priv, ml_kem_seed, ml_dsa_seed
                 FROM local_identity WHERE id = 1",
                [],
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
            .optional()?;

        row.map(|(device_id, device_name, x25519_priv, ml_kem_seed, ml_dsa_seed)| {
            LocalIdentity::from_persisted(
                &device_name,
                &device_id,
                blob_to_array(&x25519_priv)?,
                blob_to_array(&ml_kem_seed)?,
                blob_to_array(&ml_dsa_seed)?,
            )
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Blob,
                    e.into(),
                )
            })
        })
        .transpose()
    }

    /// Returns this device's persisted identity, creating and storing one on
    /// first call. `device_id` never changes once stored; only `device_name` is
    /// updated when the caller supplies a new one.
    pub fn load_or_create_identity(&self, device_name: &str) -> rusqlite::Result<LocalIdentity> {
        if let Some(mut identity) = self.load_identity()? {
            if identity.bundle.device_name != device_name {
                self.conn.execute(
                    "UPDATE local_identity SET device_name = ?1 WHERE id = 1",
                    params![device_name],
                )?;
                identity.bundle.device_name = device_name.to_string();
            }
            return Ok(identity);
        }

        let identity = LocalIdentity::generate(device_name);
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT INTO local_identity (id, device_id, device_name, x25519_priv, ml_kem_seed, ml_dsa_seed, created_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                identity.bundle.device_id,
                identity.bundle.device_name,
                identity.x25519_private.as_slice(),
                identity.ml_kem_seed.as_slice(),
                identity.ml_dsa_seed.as_slice(),
                created_at,
            ],
        )?;
        Ok(identity)
    }
}

// ============================================================================
// PHASE 2 & 3: HYBRID LOCATION UPDATE, WITH SENDER-BOUND SIG + REPLAY GUARD
// ============================================================================

mod base64_bytes {
    use base64::Engine;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Base64Visitor;

        impl<'de> serde::de::Visitor<'de> for Base64Visitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a base64 encoded string or a sequence of bytes")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                base64::engine::general_purpose::STANDARD
                    .decode(v)
                    .map_err(serde::de::Error::custom)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = Vec::new();
                while let Some(byte) = seq.next_element()? {
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(Base64Visitor)
    }
}

mod base64_array {
    use base64::Engine;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S, const N: usize>(bytes: &[u8; N], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D, const N: usize>(deserializer: D) -> Result<[u8; N], D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ArrayVisitor<const N: usize>;

        impl<'de, const N: usize> serde::de::Visitor<'de> for ArrayVisitor<N> {
            type Value = [u8; N];

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "a base64 encoded {} byte array", N)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(v)
                    .map_err(serde::de::Error::custom)?;
                decoded
                    .try_into()
                    .map_err(|_| serde::de::Error::custom(format!("expected {} bytes", N)))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = [0u8; N];
                for (i, byte) in bytes.iter_mut().enumerate() {
                    *byte = seq
                        .next_element()?
                        .ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(ArrayVisitor::<N>)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocationPayload {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: Option<f64>,
    pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LocationUpdatePackage {
    pub sender_id: String,
    /// Per-sender monotonic counter. This, not `timestamp`, is what stops replay.
    pub sequence: u64,
    #[serde(with = "base64_bytes")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "base64_bytes")]
    pub wrapped_key: Vec<u8>,
    #[serde(with = "base64_bytes")]
    pub ct_pq: Vec<u8>,
    #[serde(with = "base64_array")]
    pub x_eph: [u8; 32],
    #[serde(with = "base64_bytes")]
    pub sig: Vec<u8>,
    #[serde(with = "base64_array")]
    pub n1: [u8; 12],
    #[serde(with = "base64_array")]
    pub n2: [u8; 12],
    pub timestamp: u64,
}

/// Decodes a package exactly as it travels through the server inbox. Kept
/// separate from `receive_location_update` so a malformed message is a normal
/// error the caller can skip, not a crash.
pub fn decode_location_package(bytes: &[u8]) -> Result<LocationUpdatePackage, String> {
    serde_json::from_slice(bytes)
        .map_err(|e| format!("Failed to decode location package: {}", e))
}

/// What the UI gets after a successful decrypt: the payload plus who sent it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedLocationUpdate {
    pub sender_id: String,
    pub sender_name: String,
    pub sequence: u64,
    pub timestamp: u64,
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: Option<f64>,
}

/// Byte string that gets signed and verified. `sender_id` and `sequence` are
/// bound in here so the signature can't be replayed under a different
/// identity, and variable-length fields are length-prefixed so there is no
/// ambiguity about where one field ends and the next begins.
fn build_signed_message(
    sender_id: &str,
    sequence: u64,
    x_eph: &[u8; 32],
    ct_pq: &[u8],
    wrapped_key: &[u8],
    timestamp: u64,
) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(&(sender_id.len() as u32).to_be_bytes());
    msg.extend_from_slice(sender_id.as_bytes());
    msg.extend_from_slice(&sequence.to_be_bytes());
    msg.extend_from_slice(x_eph);
    msg.extend_from_slice(&(ct_pq.len() as u32).to_be_bytes());
    msg.extend_from_slice(ct_pq);
    msg.extend_from_slice(&(wrapped_key.len() as u32).to_be_bytes());
    msg.extend_from_slice(wrapped_key);
    msg.extend_from_slice(&timestamp.to_be_bytes());
    msg
}

/// Alice's per-device sequence counter, durably persisted to a SQLite file.
/// `synchronous = FULL` means every `reserve_next` call fsyncs before
/// returning: the number handed back is on disk before the caller can use
/// it, so it can never be handed back again after a crash. That fsync costs
/// real latency (single-digit milliseconds on a normal SSD, more on
/// spinning disk or a cheap phone eMMC) -- that's the price of the
/// guarantee, not a bug. Don't point this at ":memory:" outside tests: an
/// in-memory DB gives you back exactly the crash-unsafe behavior this
/// replaces.
pub struct SequenceStore {
    conn: Connection,
}

impl SequenceStore {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sequence_counter (
                device_id TEXT PRIMARY KEY,
                next_seq  INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Atomically reserves and durably commits the next sequence number for
    /// `device_id`, then returns it. One SQL statement: no separate
    /// read-then-write race to get wrong.
    pub fn reserve_next(&self, device_id: &str) -> rusqlite::Result<u64> {
        let new_next: i64 = self.conn.query_row(
            "INSERT INTO sequence_counter (device_id, next_seq) VALUES (?1, 1)
             ON CONFLICT(device_id) DO UPDATE SET next_seq = next_seq + 1
             RETURNING next_seq",
            params![device_id],
            |row| row.get(0),
        )?;
        Ok(new_next as u64 - 1)
    }
}

/// Bob's per-sender replay guard, durably persisted to a SQLite file for the
/// same reason as `SequenceStore`: an in-memory map forgets everything on
/// restart, which un-does the replay protection the first time the app
/// restarts or crashes.
pub struct ReplayGuard {
    conn: Connection,
}

impl ReplayGuard {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS replay_guard (
                sender_id TEXT PRIMARY KEY,
                last_seq  INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Single atomic statement: insert if this sender is new, otherwise
    /// only update if the incoming sequence is strictly greater than what's
    /// stored. If the WHERE clause fails (replay or reorder), no row is
    /// touched and RETURNING yields nothing -- verified this actually
    /// happens rather than assuming it from the SQLite docs.
    fn check_and_advance(&self, sender_id: &str, sequence: u64) -> Result<(), &'static str> {
        let accepted: Option<i64> = self
            .conn
            .query_row(
                "INSERT INTO replay_guard (sender_id, last_seq) VALUES (?1, ?2)
                 ON CONFLICT(sender_id) DO UPDATE SET last_seq = excluded.last_seq
                 WHERE excluded.last_seq > replay_guard.last_seq
                 RETURNING last_seq",
                params![sender_id, sequence as i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "replay guard storage error")?;

        match accepted {
            Some(_) => Ok(()),
            None => Err("replayed or out-of-order sequence number"),
        }
    }
}

pub fn send_location_update(
    bob_kem_pub: &EncapsulationKey,
    bob_x25519_pub_bytes: &[u8; 32],
    alice_mldsa_priv: &ml_dsa::SigningKey<MlDsa65>,
    sender_id: String,
    sequence: u64,
    payload: &[u8],
    timestamp: u64,
) -> LocationUpdatePackage {
    let mut rng = rand::rng();

    // 1. Generate ephemerals & content key.
    let mut ck = [0u8; 32];
    rand::RngExt::fill(&mut rng, &mut ck);

    let x_eph_sec = EphemeralSecret::random_from_rng(&mut rng);
    let x_eph_pub = X25519PublicKey::from(&x_eph_sec);

    // 2. Hybrid encapsulation: classical DH + PQ KEM.
    let bob_x25519_pub = X25519PublicKey::from(*bob_x25519_pub_bytes);
    let ss_classic = x_eph_sec.diffie_hellman(&bob_x25519_pub);

    let (ct_pq, ss_pq) = bob_kem_pub.encapsulate_with_rng(&mut rng);
    let ct_pq_bytes = ct_pq.as_slice().to_vec();

    // 3. Derive the wrapping key.
    let mut ikm = Vec::new();
    ikm.extend_from_slice(ss_classic.as_bytes());
    ikm.extend_from_slice(ss_pq.as_slice());
    ikm.extend_from_slice(x_eph_pub.as_bytes());
    ikm.extend_from_slice(&ct_pq_bytes);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut wrap_key = [0u8; 32];
    hk.expand(b"location_wrap", &mut wrap_key)
        .expect("HKDF expand failed");

    // 4. Encrypt payload under CK, then wrap CK under wrap_key.
    let mut n1 = [0u8; 12];
    let mut n2 = [0u8; 12];
    rand::RngExt::fill(&mut rng, &mut n1);
    rand::RngExt::fill(&mut rng, &mut n2);

    let payload_cipher = Aes256Gcm::new_from_slice(&ck).expect("invalid key len");
    let nonce_n1 = AesNonce::try_from(&n1[..]).expect("invalid nonce length");
    let ciphertext = payload_cipher
        .encrypt(&nonce_n1, payload)
        .expect("payload encryption failed");

    let wrap_cipher = Aes256Gcm::new_from_slice(&wrap_key).expect("invalid key len");
    let nonce_n2 = AesNonce::try_from(&n2[..]).expect("invalid nonce length");
    let wrapped_key = wrap_cipher
        .encrypt(&nonce_n2, ck.as_ref())
        .expect("content-key wrap failed");

    // 5. Sign sender_id + sequence + the handshake metadata.
    let msg_to_sign = build_signed_message(
        &sender_id,
        sequence,
        x_eph_pub.as_bytes(),
        &ct_pq_bytes,
        &wrapped_key,
        timestamp,
    );
    let sig = alice_mldsa_priv.sign(&msg_to_sign);

    LocationUpdatePackage {
        sender_id,
        sequence,
        ciphertext,
        wrapped_key,
        ct_pq: ct_pq_bytes,
        x_eph: *x_eph_pub.as_bytes(),
        sig: sig.to_bytes().to_vec(),
        n1,
        n2,
        timestamp,
    }
}

pub fn receive_location_update(
    package: &LocationUpdatePackage,
    bob_kem_priv: &DecapsulationKey,
    bob_x25519_priv: &StaticSecret,
    alice_mldsa_pub: &ml_dsa::VerifyingKey<MlDsa65>,
    replay_guard: &ReplayGuard,
) -> Result<Vec<u8>, &'static str> {
    // 1. Verify the signature over sender_id + sequence + handshake metadata.
    let msg_to_verify = build_signed_message(
        &package.sender_id,
        package.sequence,
        &package.x_eph,
        &package.ct_pq,
        &package.wrapped_key,
        package.timestamp,
    );

    let sig = ml_dsa::Signature::<MlDsa65>::try_from(package.sig.as_slice())
        .map_err(|_| "failed to parse signature")?;
    alice_mldsa_pub
        .verify(&msg_to_verify, &sig)
        .map_err(|_| "ML-DSA-65 signature verification failed")?;

    // 2. Only after the signature checks out, enforce replay protection.
    //    (Checking before verification would let anyone with garbage bytes
    //    burn sequence numbers and lock out a legitimate sender.)
    replay_guard.check_and_advance(&package.sender_id, package.sequence)?;

    // 3. Decapsulate: classical DH + PQ KEM.
    let x_eph_pub = X25519PublicKey::from(package.x_eph);
    let ss_classic = bob_x25519_priv.diffie_hellman(&x_eph_pub);

    let ct_pq = KemCiphertext::try_from(package.ct_pq.as_slice())
        .map_err(|_| "invalid KEM ciphertext length")?;
    let ss_pq = bob_kem_priv.decapsulate(&ct_pq);

    // 4. Reconstruct the wrap key.
    let mut ikm = Vec::new();
    ikm.extend_from_slice(ss_classic.as_bytes());
    ikm.extend_from_slice(ss_pq.as_slice());
    ikm.extend_from_slice(&package.x_eph);
    ikm.extend_from_slice(&package.ct_pq);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut wrap_key = [0u8; 32];
    hk.expand(b"location_wrap", &mut wrap_key)
        .map_err(|_| "HKDF expand failed")?;

    // 5. Unwrap CK, then decrypt the payload.
    let wrap_cipher = Aes256Gcm::new_from_slice(&wrap_key).map_err(|_| "invalid wrap key")?;
    let nonce_n2 = AesNonce::try_from(&package.n2[..]).map_err(|_| "invalid nonce")?;
    let ck = wrap_cipher
        .decrypt(&nonce_n2, package.wrapped_key.as_ref())
        .map_err(|_| "failed to unwrap content key")?;

    let payload_cipher = Aes256Gcm::new_from_slice(&ck).map_err(|_| "invalid content key")?;
    let nonce_n1 = AesNonce::try_from(&package.n1[..]).map_err(|_| "invalid nonce")?;
    payload_cipher
        .decrypt(&nonce_n1, package.ciphertext.as_ref())
        .map_err(|_| "failed to decrypt location payload")
}

/// Receives a package that just came off the wire against our own persisted
/// identity and the sender's pinned bundle, returning the decoded payload.
/// Signature verification and the replay guard happen inside
/// `receive_location_update`, before any decryption.
pub fn decrypt_from_identity(
    package: &LocationUpdatePackage,
    our_identity: &LocalIdentity,
    sender_bundle: &IdentityBundle,
    replay_guard: &ReplayGuard,
) -> Result<LocationPayload, String> {
    let plaintext = receive_location_update(
        package,
        &our_identity
            .kem_decapsulation_key()
            .map_err(|e| format!("invalid local ML-KEM key: {}", e))?,
        &our_identity.x25519_secret(),
        &verifying_key_from_bundle(sender_bundle)?,
        replay_guard,
    )
    .map_err(|e| e.to_string())?;

    serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Failed to decode location payload: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ml_dsa::Keypair;

    #[test]
    fn round_trip_and_replay_is_rejected() {
        let mut rng = rand::rng();

        // Set up Bob's static keys (normally done once, at Phase 1 pairing).
        let bob_kem_dk: DecapsulationKey = Generate::generate_from_rng(&mut rng);
        let bob_kem_ek = bob_kem_dk.encapsulation_key().clone();
        let bob_x25519_priv = StaticSecret::random_from_rng(&mut rng);
        let bob_x25519_pub = X25519PublicKey::from(&bob_x25519_priv);

        // Set up Alice's static signing key.
        let alice_sk = ml_dsa::SigningKey::<MlDsa65>::generate_from_rng(&mut rng);
        let alice_vk = alice_sk.verifying_key();

        // ":memory:" is fine here, the test doesn't care about surviving a
        // restart -- see `sequence_and_replay_state_survive_a_restart` below
        // for that guarantee.
        let seq_store = SequenceStore::open(":memory:").unwrap();
        let guard = ReplayGuard::open(":memory:").unwrap();

        let payload = b"lat=45.90,lon=6.13";
        let package = send_location_update(
            &bob_kem_ek,
            bob_x25519_pub.as_bytes(),
            &alice_sk,
            "alice".to_string(),
            seq_store.reserve_next("alice-phone").unwrap(),
            payload,
            1_752_000_000,
        );

        let decrypted =
            receive_location_update(&package, &bob_kem_dk, &bob_x25519_priv, &alice_vk, &guard)
                .expect("first delivery should succeed");
        assert_eq!(decrypted, payload);

        // Bob (or the server) replays the exact same package again.
        let replay_result =
            receive_location_update(&package, &bob_kem_dk, &bob_x25519_priv, &alice_vk, &guard);
        assert!(replay_result.is_err(), "replayed package must be rejected");

        // A genuinely new update with the next sequence number still works.
        let package2 = send_location_update(
            &bob_kem_ek,
            bob_x25519_pub.as_bytes(),
            &alice_sk,
            "alice".to_string(),
            seq_store.reserve_next("alice-phone").unwrap(),
            b"lat=45.91,lon=6.14",
            1_752_000_030,
        );
        assert!(receive_location_update(
            &package2,
            &bob_kem_dk,
            &bob_x25519_priv,
            &alice_vk,
            &guard
        )
        .is_ok());
    }

    /// This is the test that actually justifies the SQLite rewrite: proves
    /// the sequence counter and replay guard both keep their state across a
    /// simulated process restart, instead of just asserting it in a comment.
    #[test]
    fn sequence_and_replay_state_survive_a_restart() {
        let seq_path = std::env::temp_dir().join(format!(
            "seq_restart_test_{}_{}.sqlite3",
            std::process::id(),
            line!()
        ));
        let guard_path = std::env::temp_dir().join(format!(
            "guard_restart_test_{}_{}.sqlite3",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&seq_path);
        let _ = std::fs::remove_file(&guard_path);

        // "Run 1": reserve two sequence numbers, accept the second one.
        {
            let seq_store = SequenceStore::open(seq_path.to_str().unwrap()).unwrap();
            let guard = ReplayGuard::open(guard_path.to_str().unwrap()).unwrap();
            assert_eq!(seq_store.reserve_next("alice-phone").unwrap(), 0);
            assert_eq!(seq_store.reserve_next("alice-phone").unwrap(), 1);
            assert!(guard.check_and_advance("alice", 1).is_ok());
            // both `seq_store` and `guard` are dropped here, simulating the
            // process dying
        }

        // "Run 2": reopen the same files, simulating a restart.
        {
            let seq_store = SequenceStore::open(seq_path.to_str().unwrap()).unwrap();
            let guard = ReplayGuard::open(guard_path.to_str().unwrap()).unwrap();

            // Counter picked up where it left off instead of resetting to 0.
            assert_eq!(seq_store.reserve_next("alice-phone").unwrap(), 2);

            // Guard still remembers sequence 1 was already accepted.
            assert!(
                guard.check_and_advance("alice", 1).is_err(),
                "replay guard must not forget state across a restart"
            );
            assert!(guard.check_and_advance("alice", 2).is_ok());
        }

        let _ = std::fs::remove_file(&seq_path);
        let _ = std::fs::remove_file(&guard_path);
    }

    fn temp_test_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "locatorr_test_{}_{}.sqlite3",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn pq_identity_bundle_carries_real_keys() {
        let identity = LocalIdentity::generate("alice-phone");

        assert_eq!(identity.bundle.ml_kem_pub.len(), ML_KEM_768_PUBLIC_KEY_LEN);
        assert_eq!(identity.bundle.ml_dsa_pub.len(), ML_DSA_65_PUBLIC_KEY_LEN);
        assert_eq!(identity.bundle.device_id.len(), 16);
        assert_eq!(identity.ml_kem_seed.len(), ML_KEM_768_SEED_LEN);
        assert_eq!(identity.ml_dsa_seed.len(), ML_DSA_65_SEED_LEN);

        // The bundle has to survive the pairing exchange, which is JSON.
        let json = serde_json::to_vec(&identity.bundle).unwrap();
        let round_tripped: IdentityBundle = serde_json::from_slice(&json).unwrap();
        assert_eq!(round_tripped.device_id, identity.bundle.device_id);
        assert_eq!(round_tripped.x25519_pub, identity.bundle.x25519_pub);
        assert_eq!(round_tripped.ml_kem_pub, identity.bundle.ml_kem_pub);
        assert_eq!(round_tripped.ml_dsa_pub, identity.bundle.ml_dsa_pub);

        // The public keys the peer sees must actually be usable.
        assert!(encapsulation_key_from_bundle(&round_tripped).is_ok());
        assert!(verifying_key_from_bundle(&round_tripped).is_ok());
    }

    #[test]
    fn identity_is_stable_across_reopen() {
        let path = temp_test_path("stable_identity");
        let _ = std::fs::remove_file(&path);

        let (device_id, kem_pub, dsa_pub) = {
            let store = ContactStore::open(path.to_str().unwrap()).unwrap();
            let identity = store.load_or_create_identity("alice-phone").unwrap();
            (
                identity.bundle.device_id.clone(),
                identity.bundle.ml_kem_pub.clone(),
                identity.bundle.ml_dsa_pub.clone(),
            )
        };

        let store = ContactStore::open(path.to_str().unwrap()).unwrap();
        let loaded = store
            .load_identity()
            .unwrap()
            .expect("identity should be persisted");
        assert_eq!(loaded.bundle.device_id, device_id);
        assert_eq!(loaded.bundle.ml_kem_pub, kem_pub);
        assert_eq!(loaded.bundle.ml_dsa_pub, dsa_pub);

        // Renaming the device must not rotate the identity.
        let renamed = store.load_or_create_identity("alice-laptop").unwrap();
        assert_eq!(renamed.bundle.device_id, device_id);
        assert_eq!(renamed.bundle.device_name, "alice-laptop");
        assert_eq!(renamed.bundle.x25519_pub, loaded.bundle.x25519_pub);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn end_to_end_send_between_paired_identities() {
        let alice = LocalIdentity::generate("alice");
        let bob = LocalIdentity::generate("bob");

        // What pairing actually transports: JSON-serialized bundles.
        let bob_as_alice_sees: IdentityBundle =
            serde_json::from_slice(&serde_json::to_vec(&bob.bundle).unwrap()).unwrap();
        let alice_as_bob_sees: IdentityBundle =
            serde_json::from_slice(&serde_json::to_vec(&alice.bundle).unwrap()).unwrap();

        let seq_store = SequenceStore::open(":memory:").unwrap();
        let guard = ReplayGuard::open(":memory:").unwrap();

        let payload = LocationPayload {
            latitude: 45.90,
            longitude: 6.13,
            accuracy_m: Some(8.0),
            timestamp: 1_752_000_000,
        };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();

        let package = send_location_update(
            &encapsulation_key_from_bundle(&bob_as_alice_sees).unwrap(),
            &bob_as_alice_sees.x25519_pub,
            &alice.signing_key(),
            alice.bundle.device_id.clone(),
            seq_store.reserve_next(&alice.bundle.device_id).unwrap(),
            &payload_bytes,
            payload.timestamp,
        );

        let decrypted = receive_location_update(
            &package,
            &bob.kem_decapsulation_key().unwrap(),
            &bob.x25519_secret(),
            &verifying_key_from_bundle(&alice_as_bob_sees).unwrap(),
            &guard,
        )
        .expect("bob should decrypt alice's update");
        assert_eq!(decrypted, payload_bytes);

        // Replaying the exact same package must be rejected.
        let replay = receive_location_update(
            &package,
            &bob.kem_decapsulation_key().unwrap(),
            &bob.x25519_secret(),
            &verifying_key_from_bundle(&alice_as_bob_sees).unwrap(),
            &guard,
        );
        assert!(replay.is_err(), "replayed package must be rejected");
    }

    #[test]
    fn location_package_survives_json_transport() {
        let alice = LocalIdentity::generate("alice");
        let bob = LocalIdentity::generate("bob");
        let guard = ReplayGuard::open(":memory:").unwrap();

        let payload = LocationPayload {
            latitude: -12.34,
            longitude: 56.78,
            accuracy_m: None,
            timestamp: 1_752_000_000,
        };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();

        let package = send_location_update(
            &encapsulation_key_from_bundle(&bob.bundle).unwrap(),
            &bob.bundle.x25519_pub,
            &alice.signing_key(),
            alice.bundle.device_id.clone(),
            0,
            &payload_bytes,
            payload.timestamp,
        );

        // This exercises the base64 serde on both `Vec<u8>` and fixed arrays.
        let wire = serde_json::to_vec(&package).unwrap();
        let decoded: LocationUpdatePackage = serde_json::from_slice(&wire).unwrap();
        assert_eq!(decoded.sender_id, package.sender_id);
        assert_eq!(decoded.x_eph, package.x_eph);
        assert_eq!(decoded.n1, package.n1);
        assert_eq!(decoded.n2, package.n2);
        assert_eq!(decoded.wrapped_key, package.wrapped_key);

        let decrypted = receive_location_update(
            &decoded,
            &bob.kem_decapsulation_key().unwrap(),
            &bob.x25519_secret(),
            &verifying_key_from_bundle(&alice.bundle).unwrap(),
            &guard,
        )
        .expect("deserialized package should still decrypt");
        assert_eq!(decrypted, payload_bytes);
    }

    #[test]
    fn validate_peer_bundle_rejects_missing_pq_keys() {
        let mut legacy_bundle = LocalIdentity::generate("legacy").bundle;
        legacy_bundle.ml_kem_pub.clear();
        legacy_bundle.ml_dsa_pub.clear();

        let err = validate_peer_bundle(&legacy_bundle).unwrap_err();
        assert!(
            err.contains("re-pair"),
            "error should tell the user how to recover, got: {}",
            err
        );

        let modern = LocalIdentity::generate("modern");
        assert!(validate_peer_bundle(&modern.bundle).is_ok());
    }

    #[test]
    fn polled_package_decrypts_against_persisted_identity() {
        let alice = LocalIdentity::generate("alice");
        let bob = LocalIdentity::generate("bob");
        let guard = ReplayGuard::open(":memory:").unwrap();

        let payload = LocationPayload {
            latitude: 51.5,
            longitude: -0.12,
            accuracy_m: Some(5.0),
            timestamp: 1_752_000_123,
        };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();

        let package = send_location_update(
            &encapsulation_key_from_bundle(&bob.bundle).unwrap(),
            &bob.bundle.x25519_pub,
            &alice.signing_key(),
            alice.bundle.device_id.clone(),
            7,
            &payload_bytes,
            payload.timestamp,
        );

        // Exactly what travels through the server inbox.
        let wire = serde_json::to_vec(&package).unwrap();
        let decoded = decode_location_package(&wire).unwrap();
        assert_eq!(decoded.sequence, 7);

        let received = decrypt_from_identity(&decoded, &bob, &alice.bundle, &guard)
            .expect("bob should decrypt alice's polled update");
        assert_eq!(received, payload);
    }

    #[test]
    fn decrypt_from_identity_rejects_replay_and_wrong_sender() {
        let alice = LocalIdentity::generate("alice");
        let bob = LocalIdentity::generate("bob");
        let mallory = LocalIdentity::generate("mallory");
        let guard = ReplayGuard::open(":memory:").unwrap();

        let payload_bytes = serde_json::to_vec(&LocationPayload {
            latitude: 1.0,
            longitude: 2.0,
            accuracy_m: None,
            timestamp: 1_752_000_000,
        })
        .unwrap();

        let package = send_location_update(
            &encapsulation_key_from_bundle(&bob.bundle).unwrap(),
            &bob.bundle.x25519_pub,
            &alice.signing_key(),
            alice.bundle.device_id.clone(),
            0,
            &payload_bytes,
            1_752_000_000,
        );

        // First delivery succeeds and advances the replay guard.
        assert!(decrypt_from_identity(&package, &bob, &alice.bundle, &guard).is_ok());

        // The same package again is a replay.
        assert!(
            decrypt_from_identity(&package, &bob, &alice.bundle, &guard).is_err(),
            "replayed package must be rejected"
        );

        // Alice's signature cannot verify against Mallory's pinned key; a fresh
        // guard keeps this from tripping the replay check instead.
        let fresh_guard = ReplayGuard::open(":memory:").unwrap();
        assert!(
            decrypt_from_identity(&package, &bob, &mallory.bundle, &fresh_guard).is_err(),
            "package must not verify against the wrong sender"
        );
    }
}
