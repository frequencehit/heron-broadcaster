// SPDX-License-Identifier: AGPL-3.0-or-later

//! Renders a medley (`docs/medleys.md`): decodes each part (a span of a
//! song or liner file), brings it to the station's loudness target, mixes
//! them ([`heron_core::medley::mix`]) and writes the result into the
//! library, replacing the medley's previous file in place. The scanner then
//! finds it, new or changed, and the API links it to the medley.

use std::path::{Path, PathBuf};

use heron_analysis::Decoder;
use heron_core::medley::{self, CHANNELS, Part, PartAudio, PartKind, Recipe, SAMPLE_RATE};
use heron_core::playout::db_to_gain;
use heron_core::{LibraryPath, PlayoutSettings, TrackPlayout};
use heron_dsp::StreamResampler;
use heron_library::lock::LibraryLock;
use serde::Serialize;
use sqlx::MySqlPool;

use crate::journal::Roots;

/// Folder of the rendered medleys, in the library.
pub use heron_core::medley::{FOLDER, medley_path};

#[derive(Debug, Serialize)]
pub struct RenderReport {
    pub medley_id: u64,
    /// Relative to the library root.
    pub path: String,
    pub duration_ms: u64,
    pub parts: usize,
}

#[derive(Debug, sqlx::FromRow)]
struct PartRow {
    kind: String,
    path: String,
    from_ms: u32,
    to_ms: u32,
    shift_ms: i32,
    move_ms: i32,
    /// The file's length, which the extensions under liners stop at.
    duration_ms: Option<u32>,
    integrated_lufs: Option<f64>,
    true_peak_db: Option<f64>,
}

async fn recipe(pool: &MySqlPool, medley_id: u64) -> Result<Recipe, String> {
    let recipe: Option<String> =
        sqlx::query_scalar("SELECT CAST(recipe AS CHAR) FROM medleys WHERE id = ?")
            .bind(medley_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
    serde_json::from_str(&recipe.ok_or("the medley was deleted")?)
        .map_err(|error| format!("invalid recipe: {error}"))
}

/// Decodes the parts and mixes them, off the async runtime.
async fn mix_rows(
    pool: &MySqlPool,
    medley_id: u64,
    roots: &Roots,
    recipe: Recipe,
    rows: Vec<PartRow>,
) -> Result<(Vec<f32>, medley::Layout), String> {
    if rows.is_empty() {
        return Err("the medley has no parts: build it first".to_owned());
    }
    let settings: PlayoutSettings = {
        // Its first station's crossfades: one file for all its stations.
        let json: Option<String> = sqlx::query_scalar(
            "SELECT CAST(s.value AS CHAR) FROM station_settings s
             JOIN medley_stations ms ON ms.station_id = s.station_id
             WHERE s.name = ? AND ms.medley_id = ?
             ORDER BY ms.position LIMIT 1",
        )
        .bind(PlayoutSettings::SETTINGS_NAME)
        .bind(medley_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
        json.and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    };
    let library = roots.library.clone();
    let parts: Vec<Part> = rows
        .iter()
        .map(|row| Part {
            kind: kind(&row.kind),
            track_id: 0,
            from_ms: row.from_ms,
            to_ms: row.to_ms,
            shift_ms: row.shift_ms,
            move_ms: row.move_ms,
        })
        .collect();
    let lengths: Vec<Option<u32>> = rows.iter().map(|row| row.duration_ms).collect();
    let layout = medley::layout(&recipe, &parts, &lengths);
    let mixed = layout.clone();
    let audio = tokio::task::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let mut parts = Vec::with_capacity(rows.len());
        for (row, placed) in rows.iter().zip(&layout.parts) {
            let file = LibraryPath::from_stored(row.path.clone()).resolve(&library);
            let samples = decode_span(&file, placed.from_ms, placed.to_ms)
                .map_err(|error| format!("{}: {error}", row.path))?;
            let mut playout = TrackPlayout::unanalyzed(row.to_ms);
            playout.integrated_lufs = row.integrated_lufs;
            playout.true_peak_db = row.true_peak_db;
            parts.push(PartAudio {
                kind: kind(&row.kind),
                samples,
                gain: db_to_gain(playout.gain_db(&settings.loudness)),
            });
        }
        Ok(medley::mix(&recipe, &layout, &parts))
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok((audio, mixed))
}

fn kind(kind: &str) -> PartKind {
    if kind == "liner" {
        PartKind::Liner
    } else {
        PartKind::Hook
    }
}

fn duration_ms(audio: &[f32]) -> u64 {
    (audio.len() / CHANNELS) as u64 * 1_000 / u64::from(SAMPLE_RATE)
}

/// Renders the medley's saved parts into its library file.
pub async fn render(
    pool: &MySqlPool,
    roots: &Roots,
    medley_id: u64,
) -> Result<RenderReport, String> {
    let recipe = recipe(pool, medley_id).await?;
    let rows: Vec<PartRow> = sqlx::query_as(
        "SELECT CAST(p.kind AS CHAR) AS kind, t.path, p.from_ms, p.to_ms, p.shift_ms, p.move_ms,
                t.duration_ms, a.integrated_lufs, a.true_peak_db
         FROM medley_parts p
         JOIN tracks t ON t.id = p.track_id
         LEFT JOIN track_analysis a ON a.track_id = t.id
         WHERE p.medley_id = ? ORDER BY p.position",
    )
    .bind(medley_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let count = rows.len();
    let (audio, layout) = mix_rows(pool, medley_id, roots, recipe, rows).await?;
    let layout = serde_json::to_string(&layout).map_err(|error| error.to_string())?;
    let duration_ms = duration_ms(&audio);

    let path = medley_path(medley_id);
    let target = LibraryPath::from_stored(path.clone()).resolve(&roots.library);
    // The scanner must not read the file while it is replaced.
    let lock = LibraryLock::acquire(pool, "librarian")
        .await
        .map_err(|error| error.to_string())?;
    let written = tokio::task::spawn_blocking(move || {
        write_wav(&target, &audio)
            .map_err(|error| format!("cannot write {}: {error}", target.display()))
    })
    .await
    .map_err(|error| error.to_string());
    lock.release().await.map_err(|error| error.to_string())?;
    written??;

    sqlx::query(
        "UPDATE medleys SET path = ?, rendered_layout = ?, rendered_at = NOW(6) WHERE id = ?",
    )
    .bind(&path)
    .bind(&layout)
    .bind(medley_id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    tracing::info!(medley_id, %path, duration_ms, parts = count, "medley rendered");
    Ok(RenderReport {
        medley_id,
        path,
        duration_ms,
        parts: count,
    })
}

/// Mixes `parts` as a render would, into the medley's preview, outside
/// the library: nothing is saved.
pub async fn preview(
    pool: &MySqlPool,
    roots: &Roots,
    medley_id: u64,
    parts: &[Part],
) -> Result<RenderReport, String> {
    let recipe = recipe(pool, medley_id).await?;
    let mut rows = Vec::with_capacity(parts.len());
    for part in parts {
        type Row = (String, Option<u32>, Option<f64>, Option<f64>);
        let track: Option<Row> = sqlx::query_as(
            "SELECT t.path, t.duration_ms, a.integrated_lufs, a.true_peak_db FROM tracks t
             LEFT JOIN track_analysis a ON a.track_id = t.id WHERE t.id = ?",
        )
        .bind(part.track_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
        let (path, duration_ms, integrated_lufs, true_peak_db) =
            track.ok_or_else(|| format!("track {} no longer exists", part.track_id))?;
        rows.push(PartRow {
            kind: match part.kind {
                PartKind::Hook => "hook",
                PartKind::Liner => "liner",
            }
            .to_owned(),
            path,
            from_ms: part.from_ms,
            to_ms: part.to_ms,
            shift_ms: part.shift_ms,
            move_ms: part.move_ms,
            duration_ms,
            integrated_lufs,
            true_peak_db,
        });
    }
    let (audio, _) = mix_rows(pool, medley_id, roots, recipe, rows).await?;
    let duration_ms = duration_ms(&audio);
    let path = medley::preview_path(medley_id);
    let target = roots.library.join(&path);
    tokio::task::spawn_blocking(move || {
        write_wav(&target, &audio)
            .map_err(|error| format!("cannot write {}: {error}", target.display()))
    })
    .await
    .map_err(|error| error.to_string())??;
    tracing::info!(
        medley_id,
        duration_ms,
        parts = parts.len(),
        "medley preview mixed"
    );
    Ok(RenderReport {
        medley_id,
        path,
        duration_ms,
        parts: parts.len(),
    })
}

/// Decodes `from_ms..to_ms` of a file (on its own timeline) into
/// interleaved stereo at [`SAMPLE_RATE`].
pub fn decode_span(path: &Path, from_ms: u32, to_ms: u32) -> Result<Vec<f32>, String> {
    let mut decoder = Decoder::open(path).map_err(|error| error.to_string())?;
    let mut resampler: Option<StreamResampler> = None;
    let mut out = Vec::new();
    let mut stereo = Vec::new();
    // Decoded up to the end of the span, with some room for the resampler.
    let wanted = u64::from(to_ms) + 200;
    while let Some(block) = decoder.next_block().map_err(|error| error.to_string())? {
        let rate = block.rate;
        let channels = block.channels.max(1);
        stereo.clear();
        for frame in block.samples.chunks(channels) {
            let left = frame[0];
            let right = if channels > 1 { frame[1] } else { frame[0] };
            stereo.push(left);
            stereo.push(right);
        }
        let resampler = match &mut resampler {
            Some(resampler) => resampler,
            None => resampler.insert(
                StreamResampler::new(rate, SAMPLE_RATE, CHANNELS)
                    .map_err(|error| error.to_string())?,
            ),
        };
        resampler
            .process(&stereo, &mut out)
            .map_err(|error| error.to_string())?;
        if decoder.frames() * 1_000 / u64::from(rate) > wanted {
            break;
        }
    }
    if let Some(resampler) = &mut resampler {
        resampler
            .finish(&mut out)
            .map_err(|error| error.to_string())?;
    }
    let from = medley::ms_to_frames(from_ms) * CHANNELS;
    let to = (medley::ms_to_frames(to_ms) * CHANNELS).min(out.len());
    if from >= to {
        return Err("the span is past the end of the file".to_owned());
    }
    Ok(out[from..to].to_vec())
}

/// Writes 16-bit stereo WAV next to `target` under a hidden name, then
/// renames it over `target`: readers see the old file or the new one, and a
/// track playing the old one plays on.
fn write_wav(target: &Path, audio: &[f32]) -> std::io::Result<()> {
    let folder = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder)?;
    let name = target
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("medley.wav");
    let temp: PathBuf = folder.join(format!(".{name}.part"));
    let spec = hound::WavSpec {
        channels: CHANNELS as u16,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let result = (|| -> Result<(), hound::Error> {
        let mut writer = hound::WavWriter::create(&temp, spec)?;
        for sample in audio {
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            writer.write_sample(value)?;
        }
        writer.finalize()
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(std::io::Error::other(error));
    }
    std::fs::rename(&temp, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tone(path: &Path, rate: u32, channels: u16, seconds: f64) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..(seconds * f64::from(rate)) as u64 {
            // The level tells the second: 0.1 in the first, 0.2 in the second…
            let level = 0.1 * (1 + n / u64::from(rate)) as f64;
            for _ in 0..channels {
                writer
                    .write_sample((level * f64::from(i16::MAX)) as i16)
                    .unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn spans_are_decoded_at_48_khz_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mono.wav");
        write_tone(&file, 44_100, 1, 5.0);
        let span = decode_span(&file, 2_000, 3_000).unwrap();
        assert_eq!(span.len(), 48_000 * CHANNELS);
        // The third second, on both channels (away from the resampler's edges).
        let middle = 24_000 * CHANNELS;
        assert!((span[middle] - 0.3).abs() < 0.01, "{}", span[middle]);
        assert!((span[middle + 1] - 0.3).abs() < 0.01);
        assert!(decode_span(&file, 6_000, 7_000).is_err());
    }

    #[test]
    fn the_file_is_replaced_whole() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("medleys/medley-1.wav");
        write_wav(&target, &[0.5; 4_800]).unwrap();
        write_wav(&target, &[0.25; 9_600]).unwrap();
        let reader = hound::WavReader::open(&target).unwrap();
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
        assert_eq!(reader.len(), 9_600);
        let names: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "no temporary file left: {names:?}");
    }
}
