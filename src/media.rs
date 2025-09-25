use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use commucat_proto::call::{
    AudioParameters as AudioConfig, CallMediaProfile as MediaConfig, VideoParameters as VideoConfig,
};

#[derive(Debug, Clone)]
pub struct AudioMetrics {
    pub level: f32,
    pub samples: usize,
    pub sample_rate: u32,
    pub channels: u8,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct VideoMetrics {
    pub width: u32,
    pub height: u32,
    pub frames_decoded: u64,
    pub timestamp: DateTime<Utc>,
}

pub struct MediaManager {
    audio_streams: HashMap<String, AudioStream>,
    video_streams: HashMap<String, VideoStream>,
}

impl MediaManager {
    pub fn new() -> Self {
        Self {
            audio_streams: HashMap::new(),
            video_streams: HashMap::new(),
        }
    }

    pub fn initialise_from_media(&mut self, call_id: &str, media: &MediaConfig) -> Result<()> {
        self.audio_streams
            .entry(call_id.to_string())
            .or_insert_with(|| AudioStream::from_config(&media.audio));
        if let Some(video) = media.video.as_ref() {
            self.video_streams
                .entry(call_id.to_string())
                .or_insert_with(|| VideoStream::from_config(video));
        }
        Ok(())
    }

    pub fn decode_audio(&mut self, call_id: &str, payload: &[u8]) -> Result<Option<AudioMetrics>> {
        let Some(stream) = self.audio_streams.get_mut(call_id) else {
            return Ok(None);
        };
        let metrics = stream.ingest(payload);
        Ok(Some(metrics))
    }

    pub fn decode_video(&mut self, call_id: &str, payload: &[u8]) -> Result<Option<VideoMetrics>> {
        let Some(stream) = self.video_streams.get_mut(call_id) else {
            return Ok(None);
        };
        let metrics = stream.ingest(payload);
        Ok(Some(metrics))
    }

    pub fn remove_call(&mut self, call_id: &str) {
        self.audio_streams.remove(call_id);
        self.video_streams.remove(call_id);
    }
}

struct AudioStream {
    sample_rate: u32,
    channels: u8,
    total_bytes: usize,
    rolling_level: f32,
}

impl AudioStream {
    fn from_config(config: &AudioConfig) -> Self {
        Self {
            sample_rate: config.sample_rate,
            channels: config.channels.clamp(1, 2),
            total_bytes: 0,
            rolling_level: 0.0,
        }
    }

    fn ingest(&mut self, payload: &[u8]) -> AudioMetrics {
        self.total_bytes += payload.len();
        let len = payload.len().max(1);
        let mut accum = 0.0f32;
        for sample in payload {
            accum += ((*sample as f32) - 128.0).abs();
        }
        let level = (accum / (len as f32 * 128.0)).clamp(0.0, 1.0);
        // Smooth spikes using a simple low-pass filter
        self.rolling_level = (self.rolling_level * 0.7) + (level * 0.3);

        AudioMetrics {
            level: self.rolling_level,
            samples: len * self.channels as usize,
            sample_rate: self.sample_rate,
            channels: self.channels,
            timestamp: Utc::now(),
        }
    }
}

struct VideoStream {
    width: u32,
    height: u32,
    frames_decoded: u64,
    total_bytes: usize,
}

impl VideoStream {
    fn from_config(config: &VideoConfig) -> Self {
        let resolution = config.max_resolution;
        Self {
            width: resolution.width.max(16) as u32,
            height: resolution.height.max(16) as u32,
            frames_decoded: 0,
            total_bytes: 0,
        }
    }

    fn ingest(&mut self, payload: &[u8]) -> VideoMetrics {
        self.frames_decoded += 1;
        self.total_bytes += payload.len();
        VideoMetrics {
            width: self.width,
            height: self.height,
            frames_decoded: self.frames_decoded,
            timestamp: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_stream_levels_increase_with_energy() {
        let config = AudioConfig {
            codec: commucat_proto::call::AudioCodec::Opus,
            bitrate: 16_000,
            sample_rate: 48_000,
            channels: 1,
            fec: true,
            dtx: true,
        };
        let mut stream = AudioStream::from_config(&config);
        let quiet = vec![128u8; 120];
        let loud = vec![255u8; 120];
        let quiet_level = stream.ingest(&quiet).level;
        let loud_level = stream.ingest(&loud).level;
        assert!(loud_level >= quiet_level);
    }

    #[test]
    fn video_stream_counts_frames() {
        let config = VideoConfig {
            codec: commucat_proto::call::VideoCodec::Vp8,
            max_bitrate: 500_000,
            max_resolution: commucat_proto::call::VideoResolution {
                width: 320,
                height: 180,
            },
            frame_rate: 24,
            adaptive: true,
        };
        let mut stream = VideoStream::from_config(&config);
        stream.ingest(&[0u8; 10]);
        let metrics = stream.ingest(&[0u8; 20]);
        assert_eq!(metrics.frames_decoded, 2);
        assert_eq!(metrics.width, 320);
        assert_eq!(metrics.height, 180);
        assert!(metrics.timestamp <= Utc::now());
    }

    #[test]
    fn audio_metrics_report_channels_and_samples() {
        let config = AudioConfig {
            codec: commucat_proto::call::AudioCodec::Opus,
            bitrate: 32_000,
            sample_rate: 48_000,
            channels: 2,
            fec: false,
            dtx: false,
        };
        let mut manager = MediaManager::new();
        let media = MediaConfig {
            audio: config.clone(),
            ..Default::default()
        };
        manager.initialise_from_media("call", &media).unwrap();
        let payload = vec![200u8; 256];
        let metrics = manager
            .decode_audio("call", &payload)
            .unwrap()
            .expect("audio metrics");
        assert_eq!(metrics.channels, 2);
        assert!(metrics.samples >= 256);
        assert_eq!(metrics.sample_rate, 48_000);
        assert!(metrics.timestamp <= Utc::now());
    }
}
