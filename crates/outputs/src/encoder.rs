// SPDX-License-Identifier: AGPL-3.0-or-later

//! Encoders: the master bus (f32, stereo, 48 kHz) to a compressed stream.

use heron_core::{EncoderKind, EncoderProfile};
use heron_dsp::StreamResampler;
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, MonoPcm, Quality, VbrMode};
use serde::Deserialize;

use crate::ConfigError;

/// Sample rate of the audio given to encoders.
pub const INPUT_RATE: u32 = 48_000;
/// Channels of the audio given to encoders (interleaved).
pub const INPUT_CHANNELS: usize = 2;

/// What an encoded stream contains, announced to stream servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub content_type: &'static str,
    pub sample_rate: u32,
    pub channels: u8,
    pub bitrate_kbps: Option<u16>,
}

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("MP3 encoding failed: {0}")]
    Mp3(String),
    #[error("AAC encoding failed: {0}")]
    Aac(String),
    #[error(transparent)]
    Resample(#[from] heron_dsp::resample::Error),
    /// The external FFmpeg encoder failed; it starts again after a delay.
    #[error("{0}")]
    External(String),
    /// The external FFmpeg encoder waits before starting again.
    #[error("waiting before starting ffmpeg again")]
    Waiting,
}

pub trait Encoder: Send {
    fn info(&self) -> &StreamInfo;
    /// Encodes interleaved stereo at 48 kHz and appends the result to `out`.
    fn encode(&mut self, pcm: &[f32], out: &mut Vec<u8>) -> Result<(), EncodeError>;
    /// Appends what the encoder still holds to `out`, when the stream ends
    /// (an engine handing its outputs over to another).
    fn flush(&mut self, out: &mut Vec<u8>) -> Result<(), EncodeError>;
}

/// Builds the encoder of `profile`, or explains why it cannot.
pub fn build(profile: &EncoderProfile) -> Result<Box<dyn Encoder>, ConfigError> {
    match profile.encoder {
        EncoderKind::Mp3 => Ok(Box::new(Mp3Encoder::new(profile)?)),
        EncoderKind::Aac => crate::aac::build(profile),
        EncoderKind::ExternalFfmpeg => {
            Ok(Box::new(crate::external::ExternalEncoder::new(profile)?))
        }
        kind => Err(ConfigError::Profile {
            profile: profile.name.clone(),
            reason: format!(
                "the {kind:?} encoder arrives in M11; MP3, AAC-LC and the external FFmpeg encoder are available"
            ),
        }),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mp3Mode {
    #[default]
    Cbr,
    Vbr,
}

/// `encoder_profiles.options` for MP3.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Mp3Options {
    mode: Mp3Mode,
    /// LAME's algorithm quality, 0 (best, slowest) to 9.
    quality: u8,
    /// VBR quality, 0 (best) to 9.
    vbr_quality: u8,
}

impl Default for Mp3Options {
    fn default() -> Self {
        Self {
            mode: Mp3Mode::Cbr,
            quality: 3,
            vbr_quality: 4,
        }
    }
}

/// MP3 with LAME.
pub struct Mp3Encoder {
    info: StreamInfo,
    lame: mp3lame_encoder::Encoder,
    resampler: StreamResampler,
    resampled: Vec<f32>,
    mono: Vec<f32>,
}

impl Mp3Encoder {
    pub fn new(profile: &EncoderProfile) -> Result<Self, ConfigError> {
        let invalid = |reason: String| ConfigError::Profile {
            profile: profile.name.clone(),
            reason,
        };
        let options: Mp3Options = if profile.options.is_null() {
            Mp3Options::default()
        } else {
            serde_json::from_value(profile.options.clone())
                .map_err(|error| invalid(format!("invalid options: {error}")))?
        };
        if !matches!(profile.channels, 1 | 2) {
            return Err(invalid(format!("{} channels", profile.channels)));
        }
        const RATES: [u32; 9] = [
            8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
        ];
        if !RATES.contains(&profile.sample_rate) {
            return Err(invalid(format!(
                "MP3 does not support {} Hz",
                profile.sample_rate
            )));
        }
        let bitrate_kbps = profile.bitrate_kbps.unwrap_or(128);
        let bitrate = bitrate(bitrate_kbps)
            .ok_or_else(|| invalid(format!("MP3 does not support {bitrate_kbps} kbps")))?;

        let lame_error = |error: mp3lame_encoder::BuildError| invalid(error.to_string());
        let mut builder =
            Builder::new().ok_or_else(|| invalid("cannot allocate the LAME encoder".to_owned()))?;
        builder
            .set_num_channels(profile.channels)
            .map_err(lame_error)?;
        builder
            .set_sample_rate(profile.sample_rate)
            .map_err(lame_error)?;
        builder
            .set_quality(quality(options.quality))
            .map_err(lame_error)?;
        builder.set_to_write_vbr_tag(false).map_err(lame_error)?;
        match options.mode {
            Mp3Mode::Cbr => {
                builder.set_vbr_mode(VbrMode::Off).map_err(lame_error)?;
                builder.set_brate(bitrate).map_err(lame_error)?;
            }
            Mp3Mode::Vbr => {
                builder.set_vbr_mode(VbrMode::Mtrh).map_err(lame_error)?;
                builder
                    .set_vbr_quality(quality(options.vbr_quality))
                    .map_err(lame_error)?;
            }
        }
        let lame = builder.build().map_err(lame_error)?;
        let resampler = StreamResampler::new(INPUT_RATE, profile.sample_rate, INPUT_CHANNELS)
            .map_err(|error| invalid(error.to_string()))?;
        Ok(Self {
            info: StreamInfo {
                content_type: "audio/mpeg",
                sample_rate: profile.sample_rate,
                channels: profile.channels,
                bitrate_kbps: (options.mode == Mp3Mode::Cbr).then_some(bitrate_kbps),
            },
            lame,
            resampler,
            resampled: Vec::new(),
            mono: Vec::new(),
        })
    }
}

impl Encoder for Mp3Encoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn encode(&mut self, pcm: &[f32], out: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.resampled.clear();
        self.resampler.process(pcm, &mut self.resampled)?;
        let frames = self.resampled.len() / INPUT_CHANNELS;
        if frames == 0 {
            return Ok(());
        }
        // LAME writes into the spare capacity: make room first.
        out.reserve(mp3lame_encoder::max_required_buffer_size(frames));
        if self.info.channels == 1 {
            self.mono.clear();
            self.mono.extend(
                self.resampled
                    .as_chunks::<INPUT_CHANNELS>()
                    .0
                    .iter()
                    .map(|frame| 0.5 * (frame[0] + frame[1])),
            );
            self.lame.encode_to_vec(MonoPcm(self.mono.as_slice()), out)
        } else {
            self.lame
                .encode_to_vec(InterleavedPcm(self.resampled.as_slice()), out)
        }
        .map_err(|error| EncodeError::Mp3(error.to_string()))?;
        Ok(())
    }

    fn flush(&mut self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        out.reserve(7_200);
        self.lame
            .flush_to_vec::<FlushNoGap>(out)
            .map_err(|error| EncodeError::Mp3(error.to_string()))?;
        Ok(())
    }
}

fn bitrate(kbps: u16) -> Option<Bitrate> {
    Some(match kbps {
        8 => Bitrate::Kbps8,
        16 => Bitrate::Kbps16,
        24 => Bitrate::Kbps24,
        32 => Bitrate::Kbps32,
        40 => Bitrate::Kbps40,
        48 => Bitrate::Kbps48,
        64 => Bitrate::Kbps64,
        80 => Bitrate::Kbps80,
        96 => Bitrate::Kbps96,
        112 => Bitrate::Kbps112,
        128 => Bitrate::Kbps128,
        160 => Bitrate::Kbps160,
        192 => Bitrate::Kbps192,
        224 => Bitrate::Kbps224,
        256 => Bitrate::Kbps256,
        320 => Bitrate::Kbps320,
        _ => return None,
    })
}

fn quality(value: u8) -> Quality {
    match value {
        0 => Quality::Best,
        1 => Quality::SecondBest,
        2 => Quality::NearBest,
        3 => Quality::VeryNice,
        4 => Quality::Nice,
        5 => Quality::Good,
        6 => Quality::Decent,
        7 => Quality::Ok,
        8 => Quality::SecondWorst,
        _ => Quality::Worst,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    use super::*;

    fn profile(options: serde_json::Value, sample_rate: u32, channels: u8) -> EncoderProfile {
        EncoderProfile {
            id: 1,
            name: "test".to_owned(),
            encoder: EncoderKind::Mp3,
            bitrate_kbps: Some(128),
            sample_rate,
            channels,
            options,
        }
    }

    /// Encodes `seconds` of a 1 kHz tone in 10 ms blocks.
    fn encode_tone(encoder: &mut dyn Encoder, seconds: usize) -> Vec<u8> {
        let mut mp3 = Vec::new();
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
            encoder.encode(&block, &mut mp3).unwrap();
        }
        mp3
    }

    /// Decodes an MP3 stream: sample rate, channels and frames.
    fn decode(mp3: Vec<u8>) -> (u32, usize, usize) {
        let stream = MediaSourceStream::new(Box::new(Cursor::new(mp3)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
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
    fn encodes_cbr_mp3_at_the_profile_rate() {
        let mut encoder = build(&profile(serde_json::Value::Null, 44_100, 2)).unwrap();
        assert_eq!(encoder.info().bitrate_kbps, Some(128));
        let mp3 = encode_tone(encoder.as_mut(), 5);
        // 128 kbps for 5 s, give or take LAME's buffering.
        assert!(
            (70_000..=82_000).contains(&mp3.len()),
            "{} bytes",
            mp3.len()
        );
        let (rate, channels, frames) = decode(mp3);
        assert_eq!((rate, channels), (44_100, 2));
        assert!(frames.abs_diff(5 * 44_100) < 4_000, "{frames} frames");
    }

    #[test]
    fn encodes_mono_and_vbr() {
        let options = serde_json::json!({"mode": "vbr", "vbr_quality": 5});
        let mut encoder = build(&profile(options, 22_050, 1)).unwrap();
        assert_eq!(encoder.info().bitrate_kbps, None);
        let (rate, channels, _) = decode(encode_tone(encoder.as_mut(), 2));
        assert_eq!((rate, channels), (22_050, 1));
    }

    #[test]
    fn rejects_what_mp3_cannot_do() {
        let mut bad = profile(serde_json::Value::Null, 96_000, 2);
        assert!(build(&bad).is_err());
        bad.sample_rate = 44_100;
        bad.bitrate_kbps = Some(100);
        assert!(build(&bad).is_err());
        bad.bitrate_kbps = Some(128);
        bad.options = serde_json::json!({"mode": "abr"});
        assert!(build(&bad).is_err());
        bad.options = serde_json::Value::Null;
        bad.encoder = EncoderKind::Opus;
        let error = build(&bad).err().unwrap();
        assert!(error.to_string().contains("M11"), "{error}");
    }
}
