// SPDX-License-Identifier: AGPL-3.0-or-later

//! AAC-LC with FFmpeg's native `aac` encoder, from the system's libavcodec
//! (linked dynamically, LGPL). The encoder itself is behind the `aac`
//! feature: it needs FFmpeg's development libraries to build and libavcodec
//! to run. fdk-aac is never used. See `docs/outputs.md#built-in-aac`.
//!
//! A profile is checked the same way with or without the feature, so that
//! the API, which does not link libavcodec, accepts AAC profiles. Without
//! it, the profile's encoder fails at run time with a clear reason, and the
//! other profiles are not affected.
//!
//! libavcodec gives raw AAC frames; the stream is ADTS, so each frame gets
//! its 7-byte header here.

#[cfg(feature = "aac")]
use ffmpeg::codec::Id;
#[cfg(feature = "aac")]
use ffmpeg::util::channel_layout::ChannelLayout;
#[cfg(feature = "aac")]
use ffmpeg::util::format::sample::{Sample, Type};
#[cfg(feature = "aac")]
use ffmpeg_next as ffmpeg;
use heron_core::EncoderProfile;
#[cfg(feature = "aac")]
use heron_dsp::StreamResampler;

use crate::ConfigError;
use crate::encoder::{EncodeError, Encoder, StreamInfo};
#[cfg(feature = "aac")]
use crate::encoder::{INPUT_CHANNELS, INPUT_RATE};

/// Samples per channel in one AAC-LC frame.
#[cfg(feature = "aac")]
const FRAME: usize = 1024;
/// ADTS sampling frequency indexes (ISO 14496-3).
const RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// What a valid AAC-LC profile asks for.
// Read by the encoder only: unused in a build without the `aac` feature.
#[cfg_attr(not(feature = "aac"), allow(dead_code))]
struct Settings {
    channels: usize,
    rate_index: u8,
    kbps: u16,
}

fn settings(profile: &EncoderProfile) -> Result<Settings, ConfigError> {
    let invalid = |reason: String| ConfigError::Profile {
        profile: profile.name.clone(),
        reason,
    };
    let empty = profile.options.is_null()
        || profile
            .options
            .as_object()
            .is_some_and(serde_json::Map::is_empty);
    if !empty {
        return Err(invalid("the AAC-LC encoder has no options".to_owned()));
    }
    let channels = match profile.channels {
        1 => 1,
        2 => 2,
        other => return Err(invalid(format!("{other} channels"))),
    };
    let rate_index = RATES
        .iter()
        .position(|rate| *rate == profile.sample_rate)
        .and_then(|index| u8::try_from(index).ok())
        .ok_or_else(|| invalid(format!("AAC does not support {} Hz", profile.sample_rate)))?;
    let kbps = profile
        .bitrate_kbps
        .ok_or_else(|| invalid("a bitrate".to_owned()))?;
    if !(8..=512).contains(&kbps) {
        return Err(invalid(format!("{kbps} kbps")));
    }
    Ok(Settings {
        channels,
        rate_index,
        kbps,
    })
}

fn stream_info(profile: &EncoderProfile, settings: &Settings) -> StreamInfo {
    StreamInfo {
        content_type: "audio/aac",
        sample_rate: profile.sample_rate,
        channels: profile.channels,
        bitrate_kbps: Some(settings.kbps),
    }
}

/// The AAC-LC encoder of `profile`: libavcodec's, or, in a build without
/// the `aac` feature, one that fails with a clear reason.
pub fn build(profile: &EncoderProfile) -> Result<Box<dyn Encoder>, ConfigError> {
    #[cfg(feature = "aac")]
    {
        Ok(Box::new(AacEncoder::new(profile)?))
    }
    #[cfg(not(feature = "aac"))]
    {
        Ok(Box::new(Unavailable {
            info: stream_info(profile, &settings(profile)?),
        }))
    }
}

/// Stands for the AAC-LC encoder in a build without the `aac` feature.
#[cfg(not(feature = "aac"))]
struct Unavailable {
    info: StreamInfo,
}

#[cfg(not(feature = "aac"))]
impl Encoder for Unavailable {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn encode(&mut self, _pcm: &[f32], _out: &mut Vec<u8>) -> Result<(), EncodeError> {
        Err(EncodeError::Aac(
            "this Heron was built without AAC-LC (the `aac` feature of heron-outputs)".to_owned(),
        ))
    }

    fn flush(&mut self, _out: &mut Vec<u8>) -> Result<(), EncodeError> {
        Ok(())
    }
}

/// AAC-LC, CBR.
#[cfg(feature = "aac")]
pub struct AacEncoder {
    info: StreamInfo,
    encoder: ffmpeg::codec::encoder::Audio,
    resampler: StreamResampler,
    resampled: Vec<f32>,
    /// Interleaved samples waiting for a whole frame.
    pending: Vec<f32>,
    channels: usize,
    rate_index: u8,
    /// Frames sent so far, for the presentation timestamps.
    sent: i64,
}

#[cfg(feature = "aac")]
impl AacEncoder {
    pub fn new(profile: &EncoderProfile) -> Result<Self, ConfigError> {
        let invalid = |reason: String| ConfigError::Profile {
            profile: profile.name.clone(),
            reason,
        };
        let settings = settings(profile)?;
        let (channels, kbps) = (settings.channels, settings.kbps);

        ffmpeg::init().map_err(|error| invalid(format!("cannot start libavcodec: {error}")))?;
        let codec = ffmpeg::encoder::find(Id::AAC)
            .ok_or_else(|| invalid("libavcodec has no aac encoder".to_owned()))?;
        let mut encoder = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .audio()
            .map_err(|error| invalid(error.to_string()))?;
        let rate =
            i32::try_from(profile.sample_rate).map_err(|error| invalid(error.to_string()))?;
        encoder.set_rate(rate);
        encoder.set_bit_rate(usize::from(kbps) * 1_000);
        encoder.set_format(Sample::F32(Type::Planar));
        encoder.set_channel_layout(layout(channels));
        encoder.set_time_base((1, rate));
        let encoder = encoder
            .open_as(codec)
            .map_err(|error| invalid(format!("cannot open the aac encoder: {error}")))?;

        let resampler = StreamResampler::new(INPUT_RATE, profile.sample_rate, INPUT_CHANNELS)
            .map_err(|error| invalid(error.to_string()))?;
        Ok(Self {
            info: stream_info(profile, &settings),
            encoder,
            resampler,
            resampled: Vec::new(),
            pending: Vec::new(),
            channels: settings.channels,
            rate_index: settings.rate_index,
            sent: 0,
        })
    }

    /// Sends one frame (`None`: the end of the stream) and collects what
    /// the encoder has ready.
    fn send(&mut self, samples: Option<&[f32]>, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        let error = |error: ffmpeg::Error| EncodeError::Aac(error.to_string());
        match samples {
            Some(samples) => {
                let frames = samples.len() / self.channels;
                let mut frame = ffmpeg::frame::Audio::new(
                    Sample::F32(Type::Planar),
                    frames,
                    layout(self.channels),
                );
                frame.set_rate(self.info.sample_rate);
                frame.set_pts(Some(self.sent));
                for channel in 0..self.channels {
                    for (target, source) in frame
                        .plane_mut::<f32>(channel)
                        .iter_mut()
                        .zip(samples.iter().skip(channel).step_by(self.channels))
                    {
                        *target = *source;
                    }
                }
                self.sent += i64::try_from(frames).unwrap_or(0);
                self.encoder.send_frame(&frame).map_err(error)?;
            }
            None => self.encoder.send_eof().map_err(error)?,
        }
        let mut packet = ffmpeg::Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    if let Some(data) = packet.data() {
                        out.extend_from_slice(&adts_header(
                            data.len(),
                            self.rate_index,
                            self.channels,
                        ));
                        out.extend_from_slice(data);
                    }
                }
                // Needs more audio, or the stream is finished.
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                })
                | Err(ffmpeg::Error::Eof) => return Ok(()),
                Err(other) => return Err(error(other)),
            }
        }
    }
}

#[cfg(feature = "aac")]
impl Encoder for AacEncoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn encode(&mut self, pcm: &[f32], out: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.resampled.clear();
        self.resampler.process(pcm, &mut self.resampled)?;
        if self.channels == 1 {
            self.pending.extend(
                self.resampled
                    .as_chunks::<INPUT_CHANNELS>()
                    .0
                    .iter()
                    .map(|frame| 0.5 * (frame[0] + frame[1])),
            );
        } else {
            self.pending.extend_from_slice(&self.resampled);
        }
        let size = FRAME * self.channels;
        let whole = self.pending.len() / size * size;
        if whole == 0 {
            return Ok(());
        }
        let ready: Vec<f32> = self.pending.drain(..whole).collect();
        for chunk in ready.chunks_exact(size) {
            self.send(Some(chunk), out)?;
        }
        Ok(())
    }

    fn flush(&mut self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        if !self.pending.is_empty() {
            // The last, short frame is padded with silence.
            self.pending.resize(FRAME * self.channels, 0.0);
            let last = std::mem::take(&mut self.pending);
            self.send(Some(&last), out)?;
        }
        self.send(None, out)
    }
}

#[cfg(feature = "aac")]
fn layout(channels: usize) -> ChannelLayout {
    if channels == 1 {
        ChannelLayout::MONO
    } else {
        ChannelLayout::STEREO
    }
}

#[cfg(feature = "aac")]
/// The ADTS header of one AAC-LC frame of `payload` bytes (no CRC).
fn adts_header(payload: usize, rate_index: u8, channels: usize) -> [u8; 7] {
    let length = payload + 7;
    let channels = channels as u8;
    [
        0xFF,
        0xF1,                                           // MPEG-4, layer 0, no CRC
        (1 << 6) | (rate_index << 2) | (channels >> 2), // AAC-LC (profile 1)
        ((channels & 3) << 6) | ((length >> 11) & 3) as u8,
        ((length >> 3) & 0xFF) as u8,
        (((length & 7) << 5) | 0x1F) as u8, // buffer fullness: VBR
        0xFC,
    ]
}

#[cfg(test)]
#[cfg(feature = "aac")]
mod tests {
    use std::io::Cursor;

    use heron_core::EncoderKind;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    use super::*;

    fn profile(sample_rate: u32, channels: u8) -> EncoderProfile {
        EncoderProfile {
            id: 1,
            name: "AAC 96k".to_owned(),
            encoder: EncoderKind::Aac,
            bitrate_kbps: Some(96),
            sample_rate,
            channels,
            options: serde_json::Value::Null,
        }
    }

    /// Encodes `seconds` of a 1 kHz tone in 10 ms blocks, then flushes.
    fn encode_tone(encoder: &mut AacEncoder, seconds: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut n = 0_usize;
        for _ in 0..seconds * 100 {
            let block: Vec<f32> = (0..480)
                .flat_map(|_| {
                    let t = n as f32 / INPUT_RATE as f32;
                    n += 1;
                    let value = 0.5 * (std::f32::consts::TAU * 1_000.0 * t).sin();
                    [value, value]
                })
                .collect();
            encoder.encode(&block, &mut out).unwrap();
        }
        encoder.flush(&mut out).unwrap();
        out
    }

    /// Decodes an ADTS stream: sample rate, channels and frames.
    fn decode(adts: Vec<u8>) -> (u32, usize, usize) {
        let stream = MediaSourceStream::new(Box::new(Cursor::new(adts)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("aac");
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .unwrap();
        let track = format.default_track(TrackType::Audio).unwrap();
        let params = track
            .codec_params
            .as_ref()
            .unwrap()
            .audio()
            .unwrap()
            .clone();
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .unwrap();
        let (mut rate, mut channels, mut frames) = (0, 0, 0);
        while let Ok(Some(packet)) = format.next_packet() {
            let buffer = decoder.decode(&packet).unwrap();
            rate = buffer.spec().rate();
            channels = buffer.spec().channels().count();
            frames += buffer.frames();
        }
        (rate, channels, frames)
    }

    #[test]
    fn encodes_adts_at_the_profile_rate() {
        for (rate, channels) in [(48_000, 2), (44_100, 2), (32_000, 1)] {
            let mut encoder = AacEncoder::new(&profile(rate, channels)).unwrap();
            assert_eq!(encoder.info().content_type, "audio/aac");
            let adts = encode_tone(&mut encoder, 5);
            // 96 kbps for 5 s, give or take the encoder's buffering.
            assert!(
                (45_000..=75_000).contains(&adts.len()),
                "{rate} Hz: {} bytes",
                adts.len()
            );
            let (decoded_rate, decoded_channels, frames) = decode(adts);
            assert_eq!(
                (decoded_rate, decoded_channels),
                (rate, usize::from(channels))
            );
            let expected = rate as usize * 5;
            assert!(
                frames.abs_diff(expected) < 4 * FRAME,
                "{rate} Hz: {frames} frames for {expected}"
            );
        }
    }

    #[test]
    fn refuses_what_aac_cannot_do() {
        let mut bad = profile(48_000, 2);
        bad.options = serde_json::json!({"mode": "vbr"});
        assert!(AacEncoder::new(&bad).is_err(), "options");
        assert!(AacEncoder::new(&profile(47_000, 2)).is_err(), "rate");
        assert!(AacEncoder::new(&profile(48_000, 6)).is_err(), "channels");
        let mut bad = profile(48_000, 2);
        bad.bitrate_kbps = None;
        assert!(AacEncoder::new(&bad).is_err(), "bitrate");
    }
}

#[cfg(test)]
#[cfg(not(feature = "aac"))]
mod unavailable_tests {
    use heron_core::EncoderKind;

    use super::*;

    #[test]
    fn a_build_without_aac_accepts_the_profile_and_fails_when_encoding() {
        let mut profile = EncoderProfile {
            id: 1,
            name: "AAC 96k".to_owned(),
            encoder: EncoderKind::Aac,
            bitrate_kbps: Some(96),
            sample_rate: 48_000,
            channels: 2,
            options: serde_json::Value::Null,
        };
        let mut encoder = build(&profile).unwrap();
        assert_eq!(encoder.info().content_type, "audio/aac");
        let error = encoder.encode(&[0.0; 960], &mut Vec::new()).unwrap_err();
        assert!(error.to_string().contains("without AAC-LC"), "{error}");
        profile.sample_rate = 47_000;
        assert!(build(&profile).is_err(), "still checked");
    }
}
