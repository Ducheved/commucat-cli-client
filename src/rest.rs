use anyhow::{Context, Result, anyhow};
use commucat_crypto::DeviceCertificate;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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

    pub async fn p2p_assist(
        &self,
        session: &str,
        request: &P2pAssistRequest,
    ) -> Result<P2pAssistResponse> {
        let mut endpoint = self.base.clone();
        endpoint.set_path("api/p2p/assist");
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(session)
            .json(request)
            .send()
            .await
            .context("request /api/p2p/assist")?;
        Self::parse_response(response, StatusCode::OK).await
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
            _ => Err(anyhow!(format!("request failed with status {}", status))),
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

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct P2pAssistRequest {
    #[serde(default)]
    pub peer_hint: Option<String>,
    #[serde(default)]
    pub paths: Vec<AssistPathHint>,
    #[serde(default)]
    pub prefer_reality: Option<bool>,
    #[serde(default)]
    pub fec: Option<AssistFecHint>,
    #[serde(default)]
    pub min_paths: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AssistPathHint {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub reality_fingerprint: Option<String>,
    #[serde(default)]
    pub reality_pem: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AssistFecHint {
    #[serde(default)]
    pub mtu: Option<u16>,
    #[serde(default)]
    pub repair_overhead: Option<f32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct P2pAssistResponse {
    pub noise: NoiseAdvice,
    pub pq: PqAdvice,
    pub transports: Vec<TransportAdvice>,
    pub multipath: MultipathAdvice,
    pub obfuscation: ObfuscationAdvice,
    pub security: SecuritySnapshot,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NoiseAdvice {
    pub pattern: String,
    pub prologue_hex: String,
    pub device_seed_hex: String,
    pub static_public_hex: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PqAdvice {
    pub identity_public_hex: String,
    pub signed_prekey_public_hex: String,
    pub kem_public_hex: String,
    pub signature_public_hex: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TransportAdvice {
    pub path_id: String,
    pub transport: String,
    pub resistance: String,
    pub latency: String,
    pub throughput: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MultipathAdvice {
    pub fec_mtu: u16,
    pub fec_overhead: f32,
    #[serde(default)]
    pub primary_path: Option<String>,
    #[serde(default)]
    pub sample_segments: HashMap<String, SampleBreakdown>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct SampleBreakdown {
    pub total: usize,
    pub repair: usize,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ObfuscationAdvice {
    #[serde(default)]
    pub reality_fingerprint_hex: Option<String>,
    #[serde(default)]
    pub domain_fronting: bool,
    #[serde(default)]
    pub protocol_mimicry: bool,
    #[serde(default)]
    pub tor_bridge: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecuritySnapshot {
    pub noise_handshakes: u64,
    pub pq_handshakes: u64,
    pub fec_packets: u64,
    pub multipath_sessions: u64,
    pub average_paths: f64,
    pub censorship_deflections: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test(flavor = "current_thread")]
    async fn p2p_assist_errors_for_unreachable_host() {
        let client = RestClient::new("http://127.0.0.1:9").unwrap();
        let request = P2pAssistRequest {
            peer_hint: Some("peer-1".to_string()),
            paths: vec![AssistPathHint {
                address: Some("127.0.0.1".to_string()),
                port: Some(1234),
                priority: Some(1),
                ..Default::default()
            }],
            prefer_reality: None,
            fec: Some(AssistFecHint {
                mtu: Some(1200),
                repair_overhead: Some(0.2),
            }),
            min_paths: Some(1),
        };

        assert!(client.p2p_assist("token", &request).await.is_err());
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(serialized.contains("peer-1"));
    }

    #[test]
    fn assist_response_deserializes() {
        let payload = json!({
            "noise": {
                "pattern": "NX",
                "prologue_hex": "00",
                "device_seed_hex": "11",
                "static_public_hex": "22"
            },
            "pq": {
                "identity_public_hex": "aa",
                "signed_prekey_public_hex": "bb",
                "kem_public_hex": "cc",
                "signature_public_hex": "dd"
            },
            "transports": [
                {
                    "path_id": "path-1",
                    "transport": "udp",
                    "resistance": "medium",
                    "latency": "50ms",
                    "throughput": "10mbps"
                }
            ],
            "multipath": {
                "fec_mtu": 1200,
                "fec_overhead": 0.25,
                "primary_path": "path-1",
                "sample_segments": {
                    "path-1": { "total": 10, "repair": 2 }
                }
            },
            "obfuscation": {
                "reality_fingerprint_hex": "feed",
                "domain_fronting": true,
                "protocol_mimicry": false,
                "tor_bridge": false
            },
            "security": {
                "noise_handshakes": 1,
                "pq_handshakes": 2,
                "fec_packets": 3,
                "multipath_sessions": 4,
                "average_paths": 1.5,
                "censorship_deflections": 0
            }
        });

        let response: P2pAssistResponse = serde_json::from_value(payload).unwrap();
        assert_eq!(response.transports.len(), 1);
        assert_eq!(response.multipath.sample_segments.len(), 1);
        assert!(response.obfuscation.domain_fronting);
        assert_eq!(response.security.noise_handshakes, 1);
    }
}
