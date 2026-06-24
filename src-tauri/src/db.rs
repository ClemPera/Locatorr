//! Local SQLite schema. One table per design concern: identity key material, paired contacts,
//! key-value settings, and received location shares populated by the relay HTTP client.
//!
//! KNOWN GAP: the design doc says private key material should be encrypted at rest via the OS
//! keychain. This stores it as a plain BLOB for now. The same gap is already tracked for Nooto
//! (MEK plaintext storage in SQLite); fixing it here should probably happen alongside that fix,
//! not as a one-off.

use rusqlite::Connection;

pub fn open_and_migrate(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS identity (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            ml_dsa_priv BLOB NOT NULL,
            kem_decap_priv BLOB NOT NULL,
            x25519_priv BLOB NOT NULL,
            relay_user_id TEXT
        );

        CREATE TABLE IF NOT EXISTS contacts (
            id TEXT PRIMARY KEY,
            nickname TEXT NOT NULL,
            user_id TEXT,
            ml_dsa_pub BLOB NOT NULL,
            kem_pub BLOB NOT NULL,
            x25519_pub BLOB NOT NULL,
            fingerprint TEXT NOT NULL,
            verified INTEGER NOT NULL DEFAULT 0,
            sharing INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS received_locations (
            contact_id TEXT PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
            lat REAL NOT NULL,
            lon REAL NOT NULL,
            accuracy REAL NOT NULL,
            updated_at INTEGER NOT NULL
        );
        ",
    )?;

    // Migrations for databases created before these columns existed.
    // Errors (e.g. column already exists) are ignored — these are idempotent.
    let _ = conn.execute("ALTER TABLE identity ADD COLUMN relay_user_id TEXT", []);
    let _ = conn.execute("ALTER TABLE contacts ADD COLUMN user_id TEXT", []);

    Ok(conn)
}
