use crate::hexutil::{decode_hex32, encode_hex};
use anyhow::{Context, Result, anyhow};
use commucat_crypto::{DeviceCertificate, DeviceKeyPair};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientState {
    pub device_id: String,
    pub server_url: String,
    pub domain: String,
    pub private_key: String,
    pub public_key: String,
    pub noise_pattern: String,
    pub prologue: String,
    pub tls_ca_path: Option<String>,
    pub server_static: Option<String>,
    pub insecure: bool,
    pub presence_state: String,
    pub presence_interval_secs: u64,
    pub traceparent: Option<String>,
    #[serde(default)]
    pub user_handle: Option<String>,
    #[serde(default)]
    pub user_display_name: Option<String>,
    #[serde(default)]
    pub user_avatar_url: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub last_pairing_code: Option<String>,
    #[serde(default)]
    pub last_pairing_expires_at: Option<String>,
    #[serde(default)]
    pub last_pairing_issuer_device_id: Option<String>,
    #[serde(default)]
    pub friends: Vec<FriendEntry>,
    #[serde(default)]
    pub device_certificate: Option<String>,
    #[serde(default)]
    pub device_certificate_serial: Option<u64>,
    #[serde(default)]
    pub device_certificate_issued_at: Option<i64>,
    #[serde(default)]
    pub device_certificate_expires_at: Option<i64>,
    #[serde(default)]
    pub device_ca_public: Option<String>,
}

/// Параметры формирования ClientState без чтения из файла.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FriendEntry {
    pub user_id: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
}

pub struct ClientStateParams {
    pub device_id: String,
    pub server_url: String,
    pub domain: String,
    pub keys: DeviceKeyPair,
    pub pattern: String,
    pub prologue: String,
    pub tls_ca_path: Option<String>,
    pub server_static: Option<String>,
    pub insecure: bool,
    pub presence_state: String,
    pub presence_interval_secs: u64,
    pub traceparent: Option<String>,
    pub user_handle: Option<String>,
    pub user_display_name: Option<String>,
    pub user_avatar_url: Option<String>,
    pub user_id: Option<String>,
    pub session_token: Option<String>,
    pub device_name: Option<String>,
    pub friends: Vec<FriendEntry>,
    pub device_certificate: Option<DeviceCertificate>,
    pub device_ca_public: Option<String>,
}

impl ClientState {
    pub fn load() -> Result<Self> {
        let path = state_path()?;
        let data = fs::read_to_string(path).context("state file not found")?;
        let mut state: ClientState = serde_json::from_str(&data).context("invalid state")?;
        if state.noise_pattern.is_empty() {
            state.noise_pattern = "XK".to_string();
        }
        if state.prologue.is_empty() {
            state.prologue = "commucat".to_string();
        }
        if state.presence_interval_secs == 0 {
            state.presence_interval_secs = 30;
        }
        if state.presence_state.is_empty() {
            state.presence_state = "online".to_string();
        }
        Ok(state)
    }

    pub fn save(&self) -> Result<()> {
        let path = state_path()?;
        if let Some(parent) = path.parent() {
            if parent.exists() && !parent.is_dir() {
                return Err(anyhow!(format!(
                    "state directory {} exists and is not a directory",
                    parent.display()
                )));
            }
            fs::create_dir_all(parent).context("state directory")?;
        }
        let payload = serde_json::to_string_pretty(self).context("serialize state")?;
        fs::write(path, payload).context("write state")
    }

    pub fn device_keypair(&self) -> Result<DeviceKeyPair> {
        let private = decode_hex32(&self.private_key)?;
        let public = decode_hex32(&self.public_key)?;
        Ok(DeviceKeyPair { public, private })
    }

    pub fn from_params(params: ClientStateParams) -> Self {
        let ClientStateParams {
            device_id,
            server_url,
            domain,
            keys,
            pattern,
            prologue,
            tls_ca_path,
            server_static,
            insecure,
            presence_state,
            presence_interval_secs,
            traceparent,
            user_handle,
            user_display_name,
            user_avatar_url,
            user_id,
            session_token,
            device_name,
            friends,
            device_certificate,
            device_ca_public,
        } = params;
        let device_certificate_json = device_certificate
            .as_ref()
            .map(|cert| serde_json::to_string(cert).expect("certificate serialization"));
        let device_certificate_serial = device_certificate.as_ref().map(|cert| cert.data.serial);
        let device_certificate_issued_at =
            device_certificate.as_ref().map(|cert| cert.data.issued_at);
        let device_certificate_expires_at =
            device_certificate.as_ref().map(|cert| cert.data.expires_at);
        let device_ca_public = device_ca_public.or_else(|| {
            device_certificate
                .as_ref()
                .map(|c| encode_hex(&c.data.issuer))
        });
        ClientState {
            device_id,
            server_url,
            domain,
            private_key: encode_hex(&keys.private),
            public_key: encode_hex(&keys.public),
            noise_pattern: pattern,
            prologue,
            tls_ca_path,
            server_static,
            insecure,
            presence_state,
            presence_interval_secs,
            traceparent,
            user_handle,
            user_display_name,
            user_avatar_url,
            user_id,
            session_token,
            device_name,
            last_pairing_code: None,
            last_pairing_expires_at: None,
            last_pairing_issuer_device_id: None,
            friends,
            device_certificate: device_certificate_json,
            device_certificate_serial,
            device_certificate_issued_at,
            device_certificate_expires_at,
            device_ca_public,
        }
    }

    pub fn friends(&self) -> &[FriendEntry] {
        &self.friends
    }

    pub fn set_friends(&mut self, friends: Vec<FriendEntry>) {
        self.friends = friends;
    }

    pub fn upsert_friend(&mut self, entry: FriendEntry) {
        if let Some(existing) = self
            .friends
            .iter_mut()
            .find(|friend| friend.user_id == entry.user_id)
        {
            *existing = entry;
        } else {
            self.friends.push(entry);
        }
    }

    pub fn remove_friend(&mut self, user_id: &str) -> bool {
        let before = self.friends.len();
        self.friends.retain(|friend| friend.user_id != user_id);
        before != self.friends.len()
    }

    pub fn update_keys(&mut self, keys: &DeviceKeyPair) {
        self.private_key = encode_hex(&keys.private);
        self.public_key = encode_hex(&keys.public);
        self.clear_certificate();
    }

    pub fn device_certificate(&self) -> Result<Option<DeviceCertificate>> {
        match self.device_certificate.as_ref() {
            Some(raw) => {
                let cert: DeviceCertificate = serde_json::from_str(raw)
                    .map_err(|err| anyhow!(format!("invalid device certificate: {}", err)))?;
                Ok(Some(cert))
            }
            None => Ok(None),
        }
    }

    pub fn set_certificate(&mut self, certificate: &DeviceCertificate) -> Result<()> {
        self.device_certificate = Some(serde_json::to_string(certificate)?);
        self.device_certificate_serial = Some(certificate.data.serial);
        self.device_certificate_issued_at = Some(certificate.data.issued_at);
        self.device_certificate_expires_at = Some(certificate.data.expires_at);
        self.device_ca_public = Some(encode_hex(&certificate.data.issuer));
        if self.user_id.is_none() {
            self.user_id = Some(certificate.data.user_id.clone());
        }
        Ok(())
    }

    pub fn clear_certificate(&mut self) {
        self.device_certificate = None;
        self.device_certificate_serial = None;
        self.device_certificate_issued_at = None;
        self.device_certificate_expires_at = None;
        self.device_ca_public = None;
    }
}

pub fn state_path() -> Result<PathBuf> {
    if let Ok(path) = env::var("COMMUCAT_CLIENT_HOME") {
        return Ok(PathBuf::from(path).join("client.json"));
    }

    if let Some(base_dirs) = BaseDirs::new() {
        return Ok(
            base_dirs
                .config_dir()
                .join("commucat")
                .join("client.json"),
        );
    }

    if let Ok(home) = env::var("HOME") {
        return Ok(
            Path::new(&home)
                .join(".config")
                .join("commucat")
                .join("client.json"),
        );
    }

    Err(anyhow!("unable to determine state directory"))
}

pub fn docs_path(lang: &str) -> Result<PathBuf> {
    let file = match lang {
        "ru" => "docs/README.ru.md",
        "en" => "docs/README.en.md",
        other => return Err(anyhow!(format!("unsupported language: {}", other))),
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    if path.exists() {
        Ok(path)
    } else {
        Err(anyhow!("documentation not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_state_from_keys() {
        let keys = DeviceKeyPair {
            public: [1u8; 32],
            private: [2u8; 32],
        };
        let state = ClientState::from_params(ClientStateParams {
            device_id: "device".to_string(),
            server_url: "https://example.org:8443".to_string(),
            domain: "example.org".to_string(),
            keys,
            pattern: "XK".to_string(),
            prologue: "commucat".to_string(),
            tls_ca_path: None,
            server_static: None,
            insecure: false,
            presence_state: "online".to_string(),
            presence_interval_secs: 30,
            traceparent: None,
            user_handle: Some("alice".to_string()),
            user_display_name: None,
            user_avatar_url: None,
            user_id: None,
            session_token: None,
            device_name: None,
            friends: Vec::new(),
            device_certificate: None,
            device_ca_public: None,
        });
        assert_eq!(state.device_id, "device");
        assert_eq!(state.noise_pattern, "XK");
        let pair = state.device_keypair().unwrap();
        assert_eq!(pair.public, [1u8; 32]);
        assert_eq!(pair.private, [2u8; 32]);
        assert_eq!(state.user_handle.as_deref(), Some("alice"));
    }
}
