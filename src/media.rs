use std::collections::HashMap;
use std::convert::TryFrom;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use commucat_proto::call::{
    AudioCodec, AudioParameters as AudioConfig, CallMediaProfile as MediaConfig, VideoCodec,
    VideoParameters as VideoConfig,
};
use libvpx::ffi::{
    VPX_CODEC_OK, VPX_DECODER_ABI_VERSION, vpx_codec_ctx, vpx_codec_dec_cfg,
    vpx_codec_dec_init_ver, vpx_codec_decode, vpx_codec_destroy, vpx_codec_error,
    vpx_codec_get_frame, vpx_codec_iter_t, vpx_codec_vp8_dx, vpx_codec_vp9_dx,
};
use opus::{Channels as OpusChannels, Decoder as OpusDecoder};

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

        if let Some(video) = media.video.as_ref() {
            if !self.video_streams.contains_key(call_id) {
                let stream = VideoStream::from_config(video)
                    .with_context(|| "failed to initialise VPX decoder")?;
                self.video_streams.insert(call_id.to_string(), stream);
            }
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

        if !payload.is_empty() {
            if let Ok(samples) = self.decoder.get_nb_samples(payload) {
                expected_samples = samples;
            }
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

#[derive(Copy, Clone)]
enum VpxCodecKind {
    Vp8,
    Vp9,
}

impl VpxCodecKind {
    fn name(self) -> &'static str {
        match self {
            VpxCodecKind::Vp8 => "VP8",
            VpxCodecKind::Vp9 => "VP9",
        }
    }
}

struct VpxDecoder {
    ctx: vpx_codec_ctx,
    iter: vpx_codec_iter_t,
    kind: VpxCodecKind,
}

impl VpxDecoder {
    fn new(kind: VpxCodecKind) -> Result<Self> {
        let mut ctx = MaybeUninit::<vpx_codec_ctx>::uninit();
        let cfg = MaybeUninit::<vpx_codec_dec_cfg>::zeroed();

        let iface = unsafe {
            match kind {
                VpxCodecKind::Vp8 => vpx_codec_vp8_dx(),
                VpxCodecKind::Vp9 => vpx_codec_vp9_dx(),
            }
        };

        let ret = unsafe {
            vpx_codec_dec_init_ver(
                ctx.as_mut_ptr(),
                iface,
                cfg.as_ptr(),
                0,
                VPX_DECODER_ABI_VERSION as i32,
            )
        };

        if ret != VPX_CODEC_OK {
            bail!(
                "failed to initialise {} decoder (code {})",
                kind.name(),
                ret as i32
            );
        }

        let ctx = unsafe { ctx.assume_init() };

        Ok(Self {
            ctx,
            iter: ptr::null_mut(),
            kind,
        })
    }

    fn decode_frames(&mut self, data: &[u8]) -> Result<Vec<(u32, u32)>> {
        let data_len =
            u32::try_from(data.len()).map_err(|_| anyhow!("VPX payload too large to decode"))?;
        let data_ptr = if data.is_empty() {
            ptr::null()
        } else {
            data.as_ptr()
        };

        let ret =
            unsafe { vpx_codec_decode(&mut self.ctx, data_ptr, data_len, ptr::null_mut(), 0) };

        self.iter = ptr::null_mut();

        if ret != VPX_CODEC_OK {
            let message = self
                .error_message()
                .unwrap_or_else(|| format!("code {}", ret as i32));
            bail!("{} decode failed: {}", self.kind.name(), message);
        }

        let mut frames = Vec::new();
        loop {
            let img_ptr = unsafe { vpx_codec_get_frame(&mut self.ctx, &mut self.iter) };
            if img_ptr.is_null() {
                break;
            }

            let img = unsafe { &*img_ptr };
            frames.push((img.d_w as u32, img.d_h as u32));
        }

        Ok(frames)
    }

    fn error_message(&mut self) -> Option<String> {
        let ptr = unsafe { vpx_codec_error(&mut self.ctx) };

        if ptr.is_null() {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(ptr) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }
}

impl Drop for VpxDecoder {
    fn drop(&mut self) {
        unsafe {
            vpx_codec_destroy(&mut self.ctx);
        }
    }
}

struct VideoStream {
    decoder: VpxDecoder,
    width: u32,
    height: u32,
    frames_decoded: u64,
}

impl VideoStream {
    fn from_config(config: &VideoConfig) -> Result<Self> {
        let kind = match config.codec {
            VideoCodec::Vp8 => VpxCodecKind::Vp8,
            VideoCodec::Vp9 => VpxCodecKind::Vp9,
        };

        let decoder = VpxDecoder::new(kind)?;

        Ok(Self {
            decoder,
            width: config.max_resolution.width.max(1) as u32,
            height: config.max_resolution.height.max(1) as u32,
            frames_decoded: 0,
        })
    }

    fn ingest(&mut self, payload: &[u8]) -> Result<VideoMetrics> {
        let frames = self.decoder.decode_frames(payload)?;
        let produced = frames.len() as u64;

        for (w, h) in frames {
            if w > 0 && h > 0 {
                self.width = w;
                self.height = h;
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
    use libvpx::ffi::{
        self, VPX_DL_GOOD_QUALITY, VPX_ENCODER_ABI_VERSION, vpx_codec_ctx, vpx_codec_cx_pkt_kind,
        vpx_codec_enc_cfg, vpx_codec_enc_config_default, vpx_codec_enc_init_ver, vpx_codec_encode,
        vpx_codec_get_cx_data, vpx_codec_iter_t, vpx_codec_vp8_cx, vpx_img_alloc, vpx_img_free,
    };
    use opus::{Application as OpusApplication, Encoder as OpusEncoder};
    use std::mem::MaybeUninit;
    use std::slice;

    fn encode_opus_frame(encoder: &mut OpusEncoder, pcm: &[i16]) -> Vec<u8> {
        let mut buffer = vec![0u8; 4000];
        let written = encoder.encode(pcm, &mut buffer).unwrap();
        buffer.truncate(written);
        buffer
    }

    struct TestVp8Encoder {
        ctx: vpx_codec_ctx,
        image: *mut ffi::vpx_image_t,
        iter: vpx_codec_iter_t,
    }

    impl TestVp8Encoder {
        fn new(width: u32, height: u32) -> Self {
            unsafe {
                let iface = vpx_codec_vp8_cx();
                let mut cfg = MaybeUninit::<vpx_codec_enc_cfg>::uninit();
                let ret = vpx_codec_enc_config_default(iface, cfg.as_mut_ptr(), 0);
                assert_eq!(ret, VPX_CODEC_OK);

                let mut cfg = cfg.assume_init();
                cfg.g_w = width;
                cfg.g_h = height;
                cfg.g_timebase.num = 1;
                cfg.g_timebase.den = 30;
                cfg.rc_target_bitrate = (width * height / 1000).max(64);

                let mut ctx = MaybeUninit::<vpx_codec_ctx>::uninit();
                let ret = vpx_codec_enc_init_ver(
                    ctx.as_mut_ptr(),
                    iface,
                    &cfg,
                    0,
                    VPX_ENCODER_ABI_VERSION as i32,
                );
                assert_eq!(ret, VPX_CODEC_OK);

                let ctx = ctx.assume_init();

                let image = vpx_img_alloc(
                    ptr::null_mut(),
                    ffi::vpx_img_fmt::VPX_IMG_FMT_I420,
                    width,
                    height,
                    1,
                );
                assert!(!image.is_null());

                Self {
                    ctx,
                    image,
                    iter: ptr::null_mut(),
                }
            }
        }

        fn encode_frame(&mut self, luma_value: u8, pts: i64) -> Vec<u8> {
            unsafe {
                let img = &mut *self.image;

                let y_stride = img.stride[0] as usize;
                let y_height = img.d_h as usize;
                let y_plane = slice::from_raw_parts_mut(img.planes[0], y_stride * y_height);
                y_plane.fill(luma_value);

                let uv_stride = img.stride[1] as usize;
                let chroma_height = (img.d_h as usize + 1) / 2;
                let u_plane = slice::from_raw_parts_mut(img.planes[1], uv_stride * chroma_height);
                let v_plane = slice::from_raw_parts_mut(img.planes[2], uv_stride * chroma_height);
                u_plane.fill(128);
                v_plane.fill(128);

                let ret = vpx_codec_encode(
                    &mut self.ctx,
                    img,
                    pts,
                    1,
                    0,
                    u64::from(VPX_DL_GOOD_QUALITY),
                );
                assert_eq!(ret, VPX_CODEC_OK);

                self.iter = ptr::null_mut();

                let mut encoded: Option<Vec<u8>> = None;
                loop {
                    let pkt_ptr = vpx_codec_get_cx_data(&mut self.ctx, &mut self.iter);
                    if pkt_ptr.is_null() {
                        break;
                    }

                    let pkt = &*pkt_ptr;
                    if pkt.kind == vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                        let frame = pkt.data.frame;
                        let data = slice::from_raw_parts(frame.buf as *const u8, frame.sz as usize);
                        encoded = Some(data.to_vec());
                        break;
                    }
                }

                encoded.expect("encoded VP8 frame")
            }
        }
    }

    impl Drop for TestVp8Encoder {
        fn drop(&mut self) {
            unsafe {
                vpx_img_free(self.image);
                vpx_codec_destroy(&mut self.ctx);
            }
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
        for sample in &mut loud_pcm {
            *sample = i16::MAX / 2;
        }
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
