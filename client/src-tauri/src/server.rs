use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateRoomResponse {
    pub rendezvous_id: String,
    pub expires_at: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PubkeyPayload {
    pub role: String,
    pub pubkey: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BundlePayload {
    pub role: String,
    pub bundle: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InboxPostPayload {
    pub payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InboxMessageItem {
    pub id: i64,
    pub payload: Vec<u8>,
    pub created_at: String,
}

pub struct ServerClient {
    base_url: String,
    client: reqwest::Client,
}

impl ServerClient {
    pub fn new(base_url: String) -> Self {
        let clean_url = base_url.trim_end_matches('/').to_string();
        Self {
            base_url: clean_url,
            client: reqwest::Client::new(),
        }
    }

    /// Phase 0: POST /rendezvous
    pub async fn create_rendezvous(&self) -> Result<CreateRoomResponse, String> {
        let url = format!("{}/rendezvous", self.base_url);
        let res = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to create rendezvous room: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        res.json::<CreateRoomResponse>()
            .await
            .map_err(|e| format!("Failed to parse rendezvous creation response: {}", e))
    }

    /// Phase 1: POST /rendezvous/:id
    pub async fn post_pubkey(&self, room_id: &str, role: &str, pubkey: &[u8]) -> Result<(), String> {
        let url = format!("{}/rendezvous/{}", self.base_url, room_id);
        let body = PubkeyPayload {
            role: role.to_string(),
            pubkey: pubkey.to_vec(),
        };

        let res = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to post pubkey: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        Ok(())
    }

    /// Phase 1: GET /rendezvous/:id
    pub async fn get_pubkeys(&self, room_id: &str) -> Result<Vec<PubkeyPayload>, String> {
        let url = format!("{}/rendezvous/{}", self.base_url, room_id);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to get pubkeys: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        res.json::<Vec<PubkeyPayload>>()
            .await
            .map_err(|e| format!("Failed to parse pubkeys response: {}", e))
    }

    /// Phase 1: POST /rendezvous/:id/bundle
    pub async fn post_bundle(&self, room_id: &str, role: &str, bundle: &[u8]) -> Result<(), String> {
        let url = format!("{}/rendezvous/{}/bundle", self.base_url, room_id);
        let body = BundlePayload {
            role: role.to_string(),
            bundle: bundle.to_vec(),
        };

        let res = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to post bundle: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        Ok(())
    }

    /// Phase 1: GET /rendezvous/:id/bundle
    pub async fn get_bundles(&self, room_id: &str) -> Result<Vec<BundlePayload>, String> {
        let url = format!("{}/rendezvous/{}/bundle", self.base_url, room_id);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to get bundles: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        res.json::<Vec<BundlePayload>>()
            .await
            .map_err(|e| format!("Failed to parse bundles response: {}", e))
    }

    /// Phase 2: POST /inbox/:user_id
    pub async fn post_inbox(&self, user_id: &str, payload: &[u8]) -> Result<(), String> {
        let url = format!("{}/inbox/{}", self.base_url, user_id);
        let body = InboxPostPayload {
            payload: payload.to_vec(),
        };

        let res = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to post to inbox: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        Ok(())
    }

    /// Phase 3: GET /inbox/:user_id
    pub async fn get_inbox(&self, user_id: &str) -> Result<Vec<InboxMessageItem>, String> {
        let url = format!("{}/inbox/{}", self.base_url, user_id);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to get inbox: {}", e))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Server error {}: {}", status, err_text));
        }

        res.json::<Vec<InboxMessageItem>>()
            .await
            .map_err(|e| format!("Failed to parse inbox response: {}", e))
    }
}
