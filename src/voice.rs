use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_duration_ms: u16,
    pub frames: Vec<String>,
    pub duration_ms: u32,
}

impl VoiceMessage {
    pub fn new(duration_ms: u32) -> Self {
        VoiceMessage {
            msg_type: "voice_message".to_string(),
            codec: "opus".to_string(),
            sample_rate: 48_000,
            channels: 1,
            frame_duration_ms: 20,
            frames: Vec::new(),
            duration_ms,
        }
    }

    pub fn add_frame(&mut self, frame_data: &[u8]) {
        let encoded = BASE64.encode(frame_data);
        self.frames.push(encoded);
    }

    pub fn to_bytes(&self) -> Result<Bytes, serde_json::Error> {
        let json = serde_json::to_vec(self)?;
        Ok(Bytes::from(json))
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

pub fn visualize_audio_wave(amplitude: f32, width: usize) -> String {
    let width = width.max(1);
    let normalized = (amplitude.clamp(0.0, 1.0) * width as f32) as usize;
    let filled = normalized.min(width);
    let mut wave = String::with_capacity(width + 2);
    wave.push('[');
    for i in 0..width {
        if i < filled {
            wave.push('█');
        } else {
            wave.push('░');
        }
    }
    wave.push(']');
    wave
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_message_roundtrip() {
        let mut msg = VoiceMessage::new(2_000);
        msg.add_frame(&[0, 1, 2, 3]);
        msg.add_frame(&[4, 5, 6, 7]);

        let bytes = msg.to_bytes().expect("serialize");
        let decoded = VoiceMessage::from_bytes(&bytes).expect("deserialize");
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.duration_ms, 2_000);
    }

    #[test]
    fn visualize_clamps_amplitude() {
        let short = visualize_audio_wave(0.0, 4);
        let full = visualize_audio_wave(2.0, 4);
        assert_eq!(short, "[░░░░]");
        assert_eq!(full, "[████]");
    }
}
