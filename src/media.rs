use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use commucat_proto::call::{
    AudioCodec, AudioParameters as AudioConfig, CallMediaProfile as MediaConfig, VideoCodec,
    VideoParameters as VideoConfig,
};
use opus::{Channels as OpusChannels, Decoder as OpusDecoder};
use vpx_rs::dec::CodecId as DecoderCodecId;
use vpx_rs::{Decoder, DecoderConfig};

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
        if !self.audio_streams.contains_key(call_id) {
            let stream = AudioStream::from_config(&media.audio)
                .with_context(|| "failed to initialise Opus decoder")?;
            self.audio_streams.insert(call_id.to_string(), stream);
        }

        if let Some(video) = media.video.as_ref()
            && !self.video_streams.contains_key(call_id)
        {
            let stream = VideoStream::from_config(video)
                .with_context(|| "failed to initialise VPX decoder")?;
            self.video_streams.insert(call_id.to_string(), stream);
        }

        Ok(())
    }

    pub fn decode_audio(&mut self, call_id: &str, payload: &[u8]) -> Result<Option<AudioMetrics>> {
        let Some(stream) = self.audio_streams.get_mut(call_id) else {
            return Ok(None);
        };

        stream.ingest(payload).map(Some)
    }

    pub fn decode_video(&mut self, call_id: &str, payload: &[u8]) -> Result<Option<VideoMetrics>> {
        let Some(stream) = self.video_streams.get_mut(call_id) else {
            return Ok(None);
        };

        stream.ingest(payload).map(Some)
    }

    pub fn remove_call(&mut self, call_id: &str) {
        self.audio_streams.remove(call_id);
        self.video_streams.remove(call_id);
    }
}

struct AudioStream {
    sample_rate: u32,
    channels: u8,
    decoder: OpusDecoder,
    pcm_buffer: Vec<i16>,
    rolling_level: f32,
}

impl AudioStream {
    fn from_config(config: &AudioConfig) -> Result<Self> {
        if config.codec != AudioCodec::Opus {
            bail!("unsupported audio codec: {:?}", config.codec);
        }

        let channels = match config.channels {
            1 => OpusChannels::Mono,
            2 => OpusChannels::Stereo,
            other => bail!("unsupported channel count: {}", other),
        };

        let decoder = OpusDecoder::new(config.sample_rate, channels)
            .map_err(|err| anyhow!(err.to_string()))?;

        Ok(Self {
            sample_rate: config.sample_rate,
            channels: config.channels,
            decoder,
            pcm_buffer: Vec::new(),
            rolling_level: 0.0,
        })
    }

    fn ingest(&mut self, payload: &[u8]) -> Result<AudioMetrics> {
        let fallback_samples = ((self.sample_rate / 50).max(1)) as usize;
        let mut expected_samples = fallback_samples;

        if !payload.is_empty()
            && let Ok(samples) = self.decoder.get_nb_samples(payload)
        {
            expected_samples = samples;
        }

        let required_len = expected_samples * self.channels as usize;
        if self.pcm_buffer.len() < required_len {
            self.pcm_buffer.resize(required_len, 0);
        }

        let decoded_per_channel = self
            .decoder
            .decode(payload, &mut self.pcm_buffer, false)
            .map_err(|err| anyhow!(err.to_string()))
            .context("failed to decode Opus frame")?;

        let total_samples = decoded_per_channel * self.channels as usize;
        let pcm = &self.pcm_buffer[..total_samples];

        let level = if pcm.is_empty() {
            0.0
        } else {
            let sum_sq: f32 = pcm
                .iter()
                .map(|sample| {
                    let normalised = *sample as f32 / i16::MAX as f32;
                    normalised * normalised
                })
                .sum();
            (sum_sq / pcm.len() as f32).sqrt().min(1.0)
        };

        self.rolling_level = (self.rolling_level * 0.7) + (level * 0.3);
        self.rolling_level = self.rolling_level.clamp(0.0, 1.0);

        Ok(AudioMetrics {
            level: self.rolling_level,
            samples: total_samples,
            sample_rate: self.sample_rate,
            channels: self.channels,
            timestamp: Utc::now(),
        })
    }
}

struct VideoStream {
    decoder: Decoder,
    width: u32,
    height: u32,
    frames_decoded: u64,
}

impl VideoStream {
    fn from_config(config: &VideoConfig) -> Result<Self> {
        let codec = match config.codec {
            VideoCodec::Vp8 => DecoderCodecId::VP8,
            VideoCodec::Vp9 => DecoderCodecId::VP9,
        };

        let width = config.max_resolution.width.max(1) as u32;
        let height = config.max_resolution.height.max(1) as u32;
        let decoder_config = DecoderConfig::new(codec, width, height);
        let decoder = Decoder::new(decoder_config).context("failed to create VPX decoder")?;

        Ok(Self {
            decoder,
            width,
            height,
            frames_decoded: 0,
        })
    }

    fn ingest(&mut self, payload: &[u8]) -> Result<VideoMetrics> {
        let frames = self
            .decoder
            .decode(payload)
            .context("failed to decode VPX frame")?;

        let mut produced = 0u64;
        for frame in frames {
            produced += 1;
            let width = frame.width();
            let height = frame.height();
            if width > 0 && height > 0 {
                self.width = width;
                self.height = height;
            }
        }

        self.frames_decoded += produced;

        Ok(VideoMetrics {
            width: self.width,
            height: self.height,
            frames_decoded: self.frames_decoded,
            timestamp: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opus::{Application as OpusApplication, Encoder as OpusEncoder};
    use std::num::NonZeroU32;
    use vpx_rs::enc::CodecId as EncoderCodecId;
    use vpx_rs::{
        Encoder as VpxEncoder, EncoderConfig, EncoderFrameFlags, EncodingDeadline, ImageFormat,
        Packet, RateControl, Timebase, YUVImageData,
    };

    fn encode_opus_frame(encoder: &mut OpusEncoder, pcm: &[i16]) -> Vec<u8> {
        let mut buffer = vec![0u8; 4000];
        let written = encoder.encode(pcm, &mut buffer).unwrap();
        buffer.truncate(written);
        buffer
    }

    struct TestVp8Encoder {
        encoder: VpxEncoder<u8>,
        width: u32,
        height: u32,
        buffer: Vec<u8>,
    }

    impl TestVp8Encoder {
        fn new(width: u32, height: u32) -> Self {
            let timebase = Timebase {
                num: NonZeroU32::new(1).unwrap(),
                den: NonZeroU32::new(30).unwrap(),
            };
            let mut config = EncoderConfig::<u8>::new(
                EncoderCodecId::VP8,
                width,
                height,
                timebase,
                RateControl::ConstantQuality(10),
            )
            .unwrap();
            config.lag_in_frames = 0;
            let encoder = VpxEncoder::new(config).unwrap();
            let buffer_len = ImageFormat::I420
                .buffer_len(width as usize, height as usize)
                .unwrap();
            Self {
                encoder,
                width,
                height,
                buffer: vec![0u8; buffer_len],
            }
        }

        fn encode_frame(&mut self, luma_value: u8, pts: i64) -> Vec<u8> {
            let width = self.width as usize;
            let height = self.height as usize;
            let y_len = width * height;
            let chroma_width = width / 2;
            let chroma_height = height / 2;
            let chroma_len = chroma_width * chroma_height;

            self.buffer[..y_len].fill(luma_value);
            self.buffer[y_len..y_len + chroma_len].fill(128);
            self.buffer[y_len + chroma_len..].fill(128);

            let image = YUVImageData::from_raw_data(ImageFormat::I420, width, height, &self.buffer)
                .unwrap();

            let packets = self
                .encoder
                .encode(
                    pts,
                    1,
                    image,
                    EncodingDeadline::GoodQuality,
                    EncoderFrameFlags::empty(),
                )
                .unwrap();

            for packet in packets {
                if let Packet::CompressedFrame(frame) = packet {
                    return frame.data;
                }
            }

            panic!("encoded VP8 frame not produced");
        }
    }

    fn opus_frame_samples(sample_rate: u32, channels: u8) -> Vec<i16> {
        let per_channel = (sample_rate / 50) as usize;
        vec![0i16; per_channel * channels as usize]
    }

    #[test]
    fn audio_stream_levels_increase_with_energy() {
        let config = AudioConfig {
            codec: AudioCodec::Opus,
            bitrate: 16_000,
            sample_rate: 48_000,
            channels: 1,
            fec: true,
            dtx: true,
        };

        let mut stream = AudioStream::from_config(&config).unwrap();
        let mut encoder = OpusEncoder::new(
            config.sample_rate,
            OpusChannels::Mono,
            OpusApplication::Audio,
        )
        .unwrap();

        let quiet_pcm = opus_frame_samples(config.sample_rate, config.channels);
        let quiet_packet = encode_opus_frame(&mut encoder, &quiet_pcm);

        let mut loud_pcm = quiet_pcm.clone();
        loud_pcm.fill(i16::MAX / 2);
        let loud_packet = encode_opus_frame(&mut encoder, &loud_pcm);

        let quiet_level = stream.ingest(&quiet_packet).unwrap().level;
        let loud_level = stream.ingest(&loud_packet).unwrap().level;

        assert!(loud_level > quiet_level);
    }

    #[test]
    fn audio_metrics_report_channels_and_samples() {
        let config = AudioConfig {
            codec: AudioCodec::Opus,
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

        let mut encoder = OpusEncoder::new(
            config.sample_rate,
            OpusChannels::Stereo,
            OpusApplication::Audio,
        )
        .unwrap();

        let per_channel = (config.sample_rate / 50) as usize;
        let mut pcm = Vec::with_capacity(per_channel * config.channels as usize);
        for i in 0..per_channel {
            let value = ((i % 200) as i16 - 100) * 256;
            pcm.push(value);
            pcm.push(-value);
        }

        let payload = encode_opus_frame(&mut encoder, &pcm);
        let metrics = manager
            .decode_audio("call", &payload)
            .unwrap()
            .expect("audio metrics");

        assert_eq!(metrics.channels, 2);
        assert_eq!(metrics.samples, pcm.len());
        assert_eq!(metrics.sample_rate, config.sample_rate);
        assert!(metrics.timestamp <= Utc::now());
    }

    #[test]
    fn video_stream_counts_frames() {
        let config = VideoConfig {
            codec: VideoCodec::Vp8,
            max_bitrate: 500_000,
            max_resolution: commucat_proto::call::VideoResolution {
                width: 320,
                height: 180,
            },
            frame_rate: 24,
            adaptive: true,
        };

        let mut stream = VideoStream::from_config(&config).unwrap();
        let mut encoder = TestVp8Encoder::new(320, 180);

        let frame1 = encoder.encode_frame(0x10, 0);
        let metrics1 = stream.ingest(&frame1).unwrap();
        assert_eq!(metrics1.frames_decoded, 1);
        assert_eq!(metrics1.width, 320);
        assert_eq!(metrics1.height, 180);

        let frame2 = encoder.encode_frame(0x80, 1);
        let metrics2 = stream.ingest(&frame2).unwrap();
        assert_eq!(metrics2.frames_decoded, 2);
        assert_eq!(metrics2.width, 320);
        assert_eq!(metrics2.height, 180);
    }
}
