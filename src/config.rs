use crate::hexutil::{decode_hex32, encode_hex};
use anyhow::{Context, Result, anyhow};
use commucat_crypto::DeviceKeyPair;
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
            fs::create_dir_all(parent).context("state directory")?;
        }
        let payload = serde_json::to_string_pretty(self).context("serialize state")?;
        fs::write(path, payload).context("write state")
    }

    pub fn into_device_keypair(&self) -> Result<DeviceKeyPair> {
        let private = decode_hex32(&self.private_key)?;
        let public = decode_hex32(&self.public_key)?;
        Ok(DeviceKeyPair { public, private })
    }

    pub fn with_device_keys(
        device_id: String,
        server_url: String,
        domain: String,
        keys: &DeviceKeyPair,
        pattern: String,
        prologue: String,
        tls_ca_path: Option<String>,
        server_static: Option<String>,
        insecure: bool,
        presence_state: String,
        presence_interval_secs: u64,
        traceparent: Option<String>,
    ) -> Self {
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
        }
    }
}

pub fn state_path() -> Result<PathBuf> {
    let base = if let Ok(path) = env::var("COMMUCAT_CLIENT_HOME") {
        PathBuf::from(path)
    } else {
        let home = env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
        Path::new(&home).join(".config").join("commucat")
    };
    Ok(base.join("client.json"))
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
        let state = ClientState::with_device_keys(
            "device".to_string(),
            "https://example.org:8443".to_string(),
            "example.org".to_string(),
            &keys,
            "XK".to_string(),
            "commucat".to_string(),
            None,
            None,
            false,
            "online".to_string(),
            30,
            None,
        );
        assert_eq!(state.device_id, "device");
        assert_eq!(state.noise_pattern, "XK");
        let pair = state.into_device_keypair().unwrap();
        assert_eq!(pair.public, [1u8; 32]);
        assert_eq!(pair.private, [2u8; 32]);
    }
}
