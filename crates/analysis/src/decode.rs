// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoding files with Symphonia.
//!
//! [`Decoder`] is shared by the analysis and the engine, so the engine plays
//! exactly the samples the analysis measured and cue points line up.

use std::fs::File;
use std::io::{Cursor, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use heron_core::AnalysisSettings;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::fingerprint::{Chromaprint, Fingerprinter};
use crate::{Analysis, Analyzer, Error};

/// Decodes `path` and analyzes it. CPU-bound: run it on a blocking thread.
pub fn analyze_file(path: &Path, settings: &AnalysisSettings) -> Result<Analysis, Error> {
    Ok(analyze(path, settings, false)?.0)
}

/// Like [`analyze_file`], and fingerprints the track in the same decode.
/// The fingerprint is `None` when the track is too short to get one.
pub fn analyze_and_fingerprint(
    path: &Path,
    settings: &AnalysisSettings,
) -> Result<(Analysis, Option<Chromaprint>), Error> {
    analyze(path, settings, true)
}

fn analyze(
    path: &Path,
    settings: &AnalysisSettings,
    fingerprint: bool,
) -> Result<(Analysis, Option<Chromaprint>), Error> {
    let mut decoder = Decoder::open(path)?;
    let mut state: Option<(Analyzer, Option<Fingerprinter>)> = None;
    while let Some(block) = decoder.next_block()? {
        let (analyzer, fingerprinter) = match &mut state {
            Some(state) => state,
            None => state.insert((
                Analyzer::new(block.rate, block.channels)?,
                fingerprint
                    .then(|| Fingerprinter::new(block.rate, block.channels))
                    .transpose()?,
            )),
        };
        analyzer.push(block.samples)?;
        if let Some(fingerprinter) = fingerprinter {
            fingerprinter.push(block.samples);
        }
    }
    let (analyzer, fingerprinter) = state.ok_or(Error::Empty)?;
    Ok((
        analyzer.finish(settings)?,
        fingerprinter.and_then(Fingerprinter::finish),
    ))
}

/// A block of decoded audio.
#[derive(Debug)]
pub struct Block<'a> {
    pub rate: u32,
    pub channels: usize,
    /// Interleaved samples. Never NaN or infinite, but not clamped: real
    /// masters go a little over full scale.
    pub samples: &'a [f32],
}

/// Streams the decoded audio of a file, block by block.
///
/// Decoding is gapless: encoder delay and padding (LAME headers, AAC edit
/// lists) are dropped. Some MP3s carry a broken Xing/LAME header whose frame
/// count makes gapless trimming drop every sample: when a file ends without
/// producing any audio, it is decoded again without gapless trimming.
pub struct Decoder {
    path: PathBuf,
    /// The file's content, when it was read into memory beforehand.
    contents: Option<Arc<[u8]>>,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    gapless: bool,
    /// Sample rate and channel count of the first block.
    spec: Option<(u32, usize)>,
    samples: Vec<f32>,
    frames: u64,
    skipped_packets: u64,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_with(path, None, true)
    }

    /// Decodes `contents`, the whole content of the file at `path`, read
    /// beforehand: decoding then never waits for the disk. `path` gives the
    /// format hint and names the file in logs.
    pub fn from_memory(path: &Path, contents: Arc<[u8]>) -> Result<Self, Error> {
        Self::open_with(path, Some(contents), true)
    }

    fn open_with(path: &Path, contents: Option<Arc<[u8]>>, gapless: bool) -> Result<Self, Error> {
        let source: Box<dyn symphonia::core::io::MediaSource> = match &contents {
            Some(contents) => Box::new(Cursor::new(Arc::clone(contents))),
            None => Box::new(File::open(path)?),
        };
        let stream = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(std::ffi::OsStr::to_str) {
            hint.with_extension(extension);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or(Error::NoAudioTrack)?;
        let track_id = track.id;
        let codec_params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or(Error::NoAudioTrack)?;
        let decoder_options = AudioDecoderOptions::default().gapless(gapless);
        let decoder =
            symphonia::default::get_codecs().make_audio_decoder(codec_params, &decoder_options)?;
        Ok(Self {
            path: path.to_path_buf(),
            contents,
            format,
            decoder,
            track_id,
            gapless,
            spec: None,
            samples: Vec::new(),
            frames: 0,
            skipped_packets: 0,
        })
    }

    /// Frames (samples per channel) returned so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The next block of audio, or `None` at the end of the file.
    pub fn next_block(&mut self) -> Result<Option<Block<'_>>, Error> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return self.end(),
                // Truncated files: play what could be read.
                Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                    return self.end();
                }
                Err(error) => return Err(error.into()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buffer = match self.decoder.decode(&packet) {
                Ok(buffer) => buffer,
                Err(SymphoniaError::DecodeError(reason)) => {
                    if self.skipped_packets == 0 {
                        tracing::debug!(path = %self.path.display(), reason, "undecodable packet");
                    }
                    self.skipped_packets += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };

            let rate = buffer.spec().rate();
            let channels = buffer.spec().channels().count();
            match self.spec {
                None => self.spec = Some((rate, channels)),
                Some(spec) if spec != (rate, channels) => return Err(Error::FormatChanged),
                Some(_) => {}
            }
            let len = buffer.samples_interleaved();
            if len == 0 || channels == 0 {
                continue;
            }
            self.samples.resize(len, 0.0);
            buffer.copy_to_slice_interleaved(&mut self.samples);
            for sample in &mut self.samples {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
            self.frames += (len / channels) as u64;
            return Ok(Some(Block {
                rate,
                channels,
                samples: &self.samples,
            }));
        }
    }

    fn end(&mut self) -> Result<Option<Block<'_>>, Error> {
        if self.skipped_packets > 0 {
            tracing::warn!(
                path = %self.path.display(),
                skipped_packets = self.skipped_packets,
                "skipped undecodable packets"
            );
            self.skipped_packets = 0;
        }
        if self.frames == 0 && self.gapless {
            tracing::warn!(path = %self.path.display(), "no audio with gapless decoding, retrying without");
            *self = Self::open_with(&self.path.clone(), self.contents.take(), false)?;
            return self.next_block();
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, rate: u32, segments: &[(f64, f32)]) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let mut n = 0_u64;
        for &(seconds, amplitude) in segments {
            for _ in 0..(seconds * f64::from(rate)) as u64 {
                let t = n as f64 / f64::from(rate);
                let value = amplitude * (std::f64::consts::TAU * 440.0 * t).sin() as f32;
                let sample = (value * f32::from(i16::MAX)) as i16;
                writer.write_sample(sample).unwrap();
                writer.write_sample(sample).unwrap();
                n += 1;
            }
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn analyzes_a_wav_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.wav");
        write_wav(&path, 44_100, &[(1.5, 0.0), (6.0, 0.4), (2.0, 0.0)]);

        let analysis = analyze_file(&path, &AnalysisSettings::default()).unwrap();
        assert_eq!(analysis.duration_ms, 9_500);
        assert!(analysis.cue_in_ms.abs_diff(1_500) <= 6, "{analysis:?}");
        assert!(analysis.cue_out_ms.abs_diff(7_500) <= 6, "{analysis:?}");
        assert!(analysis.integrated_lufs.is_some());
    }

    #[test]
    fn streams_every_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.wav");
        write_wav(&path, 48_000, &[(0.5, 0.0), (1.0, 0.5)]);

        let mut decoder = Decoder::open(&path).unwrap();
        let mut samples = 0;
        while let Some(block) = decoder.next_block().unwrap() {
            assert_eq!((block.rate, block.channels), (48_000, 2));
            samples += block.samples.len();
        }
        assert_eq!(samples, 2 * 72_000);
        assert_eq!(decoder.frames(), 72_000);
    }

    #[test]
    fn decodes_from_memory_like_from_disk() {
        let decode_all = |mut decoder: Decoder| {
            let mut samples = Vec::new();
            while let Some(block) = decoder.next_block().unwrap() {
                samples.extend_from_slice(block.samples);
            }
            samples
        };
        let path = Path::new(MP3_FIXTURE);
        let contents: Arc<[u8]> = std::fs::read(path).unwrap().into();
        let from_memory = decode_all(Decoder::from_memory(path, contents).unwrap());
        assert!(!from_memory.is_empty());
        assert_eq!(from_memory, decode_all(Decoder::open(path).unwrap()));
    }

    const MP3_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/silence-1s-sine-3s.mp3"
    );

    /// An ID3v2.3 tag holding `art_bytes` of cover art (APIC frame).
    fn id3v2_tag_with_cover(art_bytes: usize) -> Vec<u8> {
        let mut frame_body = b"\0image/png\0\x03\0".to_vec();
        frame_body.resize(frame_body.len() + art_bytes, 0);
        let mut frame = b"APIC".to_vec();
        frame.extend((frame_body.len() as u32).to_be_bytes());
        frame.extend([0, 0]);
        frame.extend(frame_body);

        let size = frame.len() as u32;
        let syncsafe = [
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ];
        let mut tag = b"ID3\x03\0\0".to_vec();
        tag.extend(syncsafe);
        tag.extend(frame);
        tag
    }

    #[test]
    fn analyzes_an_mp3_file() {
        let analysis = analyze_file(Path::new(MP3_FIXTURE), &AnalysisSettings::default()).unwrap();
        assert!(analysis.duration_ms.abs_diff(4_000) <= 30, "{analysis:?}");
        assert!(analysis.cue_in_ms.abs_diff(1_000) <= 30, "{analysis:?}");
    }

    #[test]
    fn skips_large_id3v2_tags() {
        // Real files often start with megabytes of embedded cover art.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("with-cover.mp3");
        let mut bytes = id3v2_tag_with_cover(3 * 1024 * 1024);
        bytes.extend(std::fs::read(MP3_FIXTURE).unwrap());
        std::fs::write(&path, bytes).unwrap();

        let analysis = analyze_file(&path, &AnalysisSettings::default()).unwrap();
        assert!(analysis.duration_ms.abs_diff(4_000) <= 30, "{analysis:?}");
        assert!(analysis.cue_in_ms.abs_diff(1_000) <= 30, "{analysis:?}");
    }

    #[test]
    fn rejects_files_that_are_not_audio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.mp3");
        std::fs::write(&path, b"definitely not an mp3 file").unwrap();
        let error = analyze_file(&path, &AnalysisSettings::default()).unwrap_err();
        assert!(!error.is_transient(), "{error}");
    }

    #[test]
    fn missing_files_are_transient_errors() {
        let error = analyze_file(
            Path::new("/nonexistent/track.flac"),
            &AnalysisSettings::default(),
        )
        .unwrap_err();
        assert!(error.is_transient());
    }
}
