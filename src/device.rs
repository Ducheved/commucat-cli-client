use crate::hexutil::encode_hex;
use anyhow::{Context, Result};
use chrono::Utc;
use commucat_crypto::DeviceKeyPair;
use getrandom::getrandom;

pub fn generate_device_id(prefix: &str) -> String {
    let ts = Utc::now().timestamp_millis();
    format!("{}-{}", prefix, ts)
}

pub fn generate_keypair() -> Result<DeviceKeyPair> {
    let mut seed = [0u8; 64];
    getrandom(&mut seed).context("sample entropy")?;
    DeviceKeyPair::from_seed(&seed).context("derive keypair")
}

pub fn describe_keys(id: &str, keys: &DeviceKeyPair) -> String {
    format!(
        "device_id={}\npublic_key={}\nprivate_key={}",
        id,
        encode_hex(&keys.public),
        encode_hex(&keys.private)
    )
}
