use anyhow::{Result, anyhow};

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(nibble(byte >> 4));
        output.push(nibble(byte & 0x0f));
    }
    output
}

pub fn decode_hex(input: &str) -> Result<Vec<u8>> {
    let cleaned = input.trim();
    if !cleaned.len().is_multiple_of(2) {
        return Err(anyhow!("invalid hex length"));
    }
    let mut output = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    for chunk in bytes.chunks(2) {
        let high = decode_digit(chunk[0])?;
        let low = decode_digit(chunk[1])?;
        output.push((high << 4) | low);
    }
    Ok(output)
}

pub fn decode_hex32(input: &str) -> Result<[u8; 32]> {
    let bytes = decode_hex(input)?;
    if bytes.len() != 32 {
        return Err(anyhow!("expected 32 bytes"));
    }
    let mut array = [0u8; 32];
    array.copy_from_slice(&bytes);
    Ok(array)
}

fn nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + value - 10) as char,
        _ => '0',
    }
}

fn decode_digit(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(10 + value - b'a'),
        b'A'..=b'F' => Ok(10 + value - b'A'),
        _ => Err(anyhow!("invalid hex digit")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data = vec![0xde, 0xad, 0xbe, 0xef];
        let encoded = encode_hex(&data);
        let decoded = decode_hex(&encoded).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn decode_hex32_ok() {
        let encoded = "00".repeat(32);
        let decoded = decode_hex32(&encoded).unwrap();
        assert_eq!(decoded, [0u8; 32]);
    }
}
