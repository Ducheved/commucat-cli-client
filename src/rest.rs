use anyhow::{Context, Result, anyhow};
use commucat_crypto::DeviceCertificate;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone)]
pub struct RestClient {
    base: Url,
    client: Client,
}

impl RestClient {
    pub fn new(server_url: &str) -> Result<Self> {
        let mut url = Url::parse(server_url).context("invalid server url")?;
        url.set_path("/");
        url.set_query(None);
        url.set_fragment(None);
        let client = Client::builder()
            .user_agent("commucat-cli-client/0.1")
            .build()
            .context("build http client")?;
        Ok(Self { base: url, client })
    }

    pub async fn server_info(&self) -> Result<ServerInfo> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/server-info");
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .context("request /api/server-info")?;
        Self::parse_response(response, StatusCode::OK).await
    }

    pub async fn create_pairing(&self, session: &str, ttl: Option<i64>) -> Result<PairingTicket> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/pair");
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(session)
            .json(&PairingRequest { ttl })
            .send()
            .await
            .context("request /api/pair")?;
        Self::parse_response(response, StatusCode::OK).await
    }

    pub async fn claim_pairing(
        &self,
        code: &str,
        device_name: Option<&str>,
    ) -> Result<PairingClaimResponse> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/pair/claim");
        let payload = PairingClaimRequest {
            pair_code: code.to_string(),
            device_name: device_name.map(ToString::to_string),
        };
        let response = self
            .client
            .post(endpoint)
            .json(&payload)
            .send()
            .await
            .context("request /api/pair/claim")?;
        Self::parse_response(response, StatusCode::OK).await
    }

    pub async fn list_devices(&self, session: &str) -> Result<Vec<DeviceEntry>> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/devices");
        let response = self
            .client
            .get(endpoint)
            .bearer_auth(session)
            .send()
            .await
            .context("request /api/devices")?;
        let envelope: DevicesEnvelope = Self::parse_response(response, StatusCode::OK).await?;
        Ok(envelope.devices)
    }

    pub async fn list_friends(&self, session: &str) -> Result<Vec<FriendEntryPayload>> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/friends");
        let response = self
            .client
            .get(endpoint)
            .bearer_auth(session)
            .send()
            .await
            .context("request /api/friends")?;
        let envelope: FriendsEnvelope = Self::parse_response(response, StatusCode::OK).await?;
        Ok(envelope.friends)
    }

    pub async fn update_friends(
        &self,
        session: &str,
        friends: &[FriendEntryPayload],
    ) -> Result<()> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/friends");
        let response = self
            .client
            .put(endpoint)
            .bearer_auth(session)
            .json(&FriendsEnvelope {
                friends: friends.to_vec(),
            })
            .send()
            .await
            .context("request /api/friends")?;
        let _: Value = Self::parse_response(response, StatusCode::OK).await?;
        Ok(())
    }

    pub async fn revoke_device(&self, session: &str, device_id: &str) -> Result<()> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/devices/revoke");
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(session)
            .json(&DeviceRevokeRequest {
                device_id: device_id.to_string(),
            })
            .send()
            .await
            .context("request /api/devices/revoke")?;
        let _: Value = Self::parse_response(response, StatusCode::OK).await?;
        Ok(())
    }

    async fn parse_response<T>(response: reqwest::Response, expected: StatusCode) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let status = response.status();
        if status == expected {
            return response.json::<T>().await.context("decode success payload");
        }
        let problem = response.json::<ProblemDetails>().await.ok();
        match problem {
            Some(details) => Err(anyhow!(details.detail.unwrap_or_else(|| {
                details
                    .title
                    .unwrap_or_else(|| format!("request failed with status {}", status))
            }))),
            None => Err(anyhow!(format!("request failed with status {}", status))),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PairingRequest {
    ttl: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairingClaimRequest {
    pair_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct PairingTicket {
    pub pair_code: String,
    pub issued_at: String,
    pub expires_at: String,
    pub ttl: i64,
    pub device_seed: String,
    #[serde(default)]
    pub issuer_device_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct PairingClaimResponse {
    pub device_id: String,
    pub private_key: String,
    pub public_key: String,
    pub seed: String,
    pub issuer_device_id: String,
    pub user: UserSummary,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub device_certificate: Option<DeviceCertificate>,
    #[serde(default)]
    pub device_ca_public: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UserSummary {
    pub id: String,
    pub handle: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DevicesEnvelope {
    devices: Vec<DeviceEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FriendsEnvelope {
    friends: Vec<FriendEntryPayload>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerInfo {
    pub domain: String,
    pub noise_public: String,
    #[serde(default)]
    pub device_ca_public: Option<String>,
    #[serde(default)]
    pub supported_patterns: Vec<String>,
    #[serde(default)]
    pub supported_versions: Vec<u16>,
    #[serde(default)]
    pub pairing: Option<ServerPairingInfo>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerPairingInfo {
    #[serde(default)]
    pub auto_approve: bool,
    #[serde(default)]
    pub pairing_ttl: i64,
    #[serde(default)]
    pub max_auto_devices: i64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct DeviceEntry {
    pub device_id: String,
    pub status: String,
    pub created_at: String,
    pub public_key: String,
    #[serde(default)]
    pub current: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FriendEntryPayload {
    pub user_id: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeviceRevokeRequest {
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct ProblemDetails {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    detail: Option<String>,
}
