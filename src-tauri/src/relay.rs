//! HTTP client for the Go relay server. All endpoints return Results with String errors.
//! Uses reqwest with rustls (no OpenSSL dependency).

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const CLIENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn client() -> Client {
    Client::builder()
        .timeout(CLIENT_TIMEOUT)
        .build()
        .expect("reqwest Client should build")
}

// --- DTOs ---

#[derive(Serialize)]
struct RegisterRequest {
    ml_dsa_pub: String,
    kem_pub: String,
    x25519_pub: String,
}

#[derive(Deserialize)]
struct RegisterResponse {
    user_id: String,
}

#[derive(Serialize)]
struct ChallengeRequest {
    user_id: String,
}

#[derive(Deserialize)]
struct ChallengeResponse {
    nonce: String,
}

#[derive(Serialize)]
struct VerifyRequest {
    user_id: String,
    signature: String,
}

#[derive(Deserialize)]
struct VerifyResponse {
    token: String,
    expires_in: i64,
}

#[derive(Serialize)]
struct PutLocationRequest {
    ciphertext: String,
    wrapped_key: String,
    kem_ciphertext: String,
    nonce: String,
    wrap_nonce: String,
}

#[derive(Deserialize)]
struct InboxItem {
    from: String,
    ciphertext: String,
    wrapped_key: String,
    kem_ciphertext: String,
    nonce: String,
    wrap_nonce: String,
    updated_at: i64,
}

/// Raw share as received from the server inbox.
#[derive(Debug, Clone)]
pub struct RawShare {
    pub from: String,
    pub ciphertext: Vec<u8>,
    pub wrapped_key: Vec<u8>,
    pub kem_ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub wrap_nonce: Vec<u8>,
    pub updated_at: i64,
}

// --- API calls ---

/// Register this device with the relay. Returns the server-assigned user_id.
pub async fn register(
    server_url: &str,
    ml_dsa_pub: &[u8],
    kem_pub: &[u8],
    x25519_pub: &[u8],
) -> Result<String, String> {
    let resp = client()
        .post(format!("{}/v1/accounts", server_url.trim_end_matches('/')))
        .json(&RegisterRequest {
            ml_dsa_pub: B64.encode(ml_dsa_pub),
            kem_pub: B64.encode(kem_pub),
            x25519_pub: B64.encode(x25519_pub),
        })
        .send()
        .await
        .map_err(|e| format!("register failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("register: server returned {}", resp.status()));
    }
    let body: RegisterResponse = resp
        .json()
        .await
        .map_err(|e| format!("register: bad response: {}", e))?;
    Ok(body.user_id)
}

/// Request an auth challenge nonce from the relay.
pub async fn request_challenge(server_url: &str, user_id: &str) -> Result<Vec<u8>, String> {
    let resp = client()
        .post(format!(
            "{}/v1/auth/challenge",
            server_url.trim_end_matches('/')
        ))
        .json(&ChallengeRequest {
            user_id: user_id.to_string(),
        })
        .send()
        .await
        .map_err(|e| format!("challenge request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("challenge: server returned {}", resp.status()));
    }
    let body: ChallengeResponse = resp
        .json()
        .await
        .map_err(|e| format!("challenge: bad response: {}", e))?;
    B64.decode(body.nonce)
        .map_err(|e| format!("challenge: bad nonce encoding: {}", e))
}

/// Submit signed nonce to get a session token.
pub async fn verify_challenge(
    server_url: &str,
    user_id: &str,
    signature: &[u8],
) -> Result<(String, i64), String> {
    let resp = client()
        .post(format!(
            "{}/v1/auth/verify",
            server_url.trim_end_matches('/')
        ))
        .json(&VerifyRequest {
            user_id: user_id.to_string(),
            signature: B64.encode(signature),
        })
        .send()
        .await
        .map_err(|e| format!("verify failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("verify: server returned {}", resp.status()));
    }
    let body: VerifyResponse = resp
        .json()
        .await
        .map_err(|e| format!("verify: bad response: {}", e))?;
    Ok((body.token, body.expires_in))
}

/// Upload an encrypted location share to a recipient.
pub async fn put_location(
    server_url: &str,
    token: &str,
    recipient_id: &str,
    share: &locatorr_crypto::envelope::Share,
) -> Result<(), String> {
    let resp = client()
        .put(format!(
            "{}/v1/locations/{}",
            server_url.trim_end_matches('/'),
            recipient_id
        ))
        .header("Authorization", format!("Bearer {}", token))
        .json(&PutLocationRequest {
            ciphertext: B64.encode(&share.ciphertext),
            wrapped_key: B64.encode(&share.wrapped_key),
            kem_ciphertext: B64.encode(&share.kem_ciphertext),
            nonce: B64.encode(share.nonce),
            wrap_nonce: B64.encode(share.wrap_nonce),
        })
        .send()
        .await
        .map_err(|e| format!("put location failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("put location: server returned {}", resp.status()));
    }
    Ok(())
}

/// Poll the inbox for new location shares.
pub async fn poll_inbox(server_url: &str, token: &str) -> Result<Vec<RawShare>, String> {
    let resp = client()
        .get(format!(
            "{}/v1/locations/inbox",
            server_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("poll inbox failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("poll inbox: server returned {}", resp.status()));
    }
    let items: Vec<InboxItem> = resp
        .json()
        .await
        .map_err(|e| format!("poll inbox: bad response: {}", e))?;

    items
        .into_iter()
        .map(|item| {
            Ok(RawShare {
                from: item.from,
                ciphertext: B64
                    .decode(item.ciphertext)
                    .map_err(|e| format!("ciphertext: {}", e))?,
                wrapped_key: B64
                    .decode(item.wrapped_key)
                    .map_err(|e| format!("wrapped_key: {}", e))?,
                kem_ciphertext: B64
                    .decode(item.kem_ciphertext)
                    .map_err(|e| format!("kem_ciphertext: {}", e))?,
                nonce: B64
                    .decode(item.nonce)
                    .map_err(|e| format!("nonce: {}", e))?,
                wrap_nonce: B64
                    .decode(item.wrap_nonce)
                    .map_err(|e| format!("wrap_nonce: {}", e))?,
                updated_at: item.updated_at,
            })
        })
        .collect()
}
