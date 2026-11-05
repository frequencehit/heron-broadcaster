# Import and analysis

How tracks get from the music library into the database, ready to air. Delivered in [M1](roadmap/m1-import-analysis.md).

```
music library ──► scanner (heron-api) ──► tracks ──► analysis_jobs ──► heron-analyzer ──► track_analysis
  (read-only)       tags, content hash                 (queue)          decode, measure,   track_fingerprints
                                                                        fingerprint
```

The scanner and the analyzer only read the library: `heron-librarian` is the only service that writes to it, when new files are imported from the buffer zone, tracks are deleted, or you ask it to reorganize the library. See [Library management](library.md).

## Scanner

The scanner runs inside `heron-api`:

- **When:** at startup, after file system changes (debounced 5 s; reads do not count), and every `HERON_SCAN_INTERVAL` seconds (default 3600) as a safety net for file systems that do not report changes (NFS, SMB). Set `HERON_LIBRARY_WATCH=false` where watching does not work.
- **What it picks up:** `mp3 m4a mp4 aac flac ogg oga opus wav aif aiff`, case-insensitive. Hidden files and folders are skipped, including macOS `._` resource forks.
- **Change detection:** each track stores the file's size and mtime. An unchanged file is not opened again, so rescanning 1,000 unchanged tracks takes under 100 ms locally.
- **New or changed files:** the scanner reads the tags (title, artist, album, album artist, genre, year, track and disc numbers, MusicBrainz recording and release, duration, via `lofty`), the audio properties (codec, bitrate, CBR or VBR from the MP3 frame headers, lossless formats as such; sample rate, channels) and a **content hash**: BLAKE3 of the size plus the first and last 64 KiB. Reading whole files would double the I/O of an import, which hurts on network shares.
- **Files still being copied:** files modified less than `HERON_IMPORT_SETTLE` seconds ago (default 5) are left for the next scan.
- **Moves and renames:** a new path whose content hash matches a track that vanished is treated as a move. The track keeps its ID, categories, history and analysis.
- **Deleted files:** the track is marked `missing_since` and keeps its categories and history. It comes back if the file reappears. If the library directory is empty while tracks are known, nothing is marked missing: an unmounted volume must not "lose" the library.
- **Paths** are stored relative to the library root with `/` separators, so every service can mount the library wherever it likes.
- **Changed files** lose their analysis and are analyzed again.
- **Tracks scanned by an older scanner** (`details_version`) are read again at the next scan for the details it did not read (bitrate, genre, …), without being analyzed again.
- **Files written by the librarian** (moved or re-tagged) are found unchanged: it records their new path and stamp itself. A scan holds the library lock, so it never runs in the middle of a move, and it skips files of a move a crashed librarian has not finished. See [Library management](library.md#coordination-with-the-scanner).

After (and during) a scan, every present track whose analysis is missing, made by another algorithm version, or made with other settings, is queued.

## Job queue

`analysis_jobs` in MariaDB, claimed with `SELECT … FOR UPDATE SKIP LOCKED`, so any number of workers and analyzer replicas can share it. See `heron_analysis::jobs`.

| Situation | What happens |
|---|---|
| Transient error (file unreadable, database down) | Retried after 30 s, then 60 s; 3 attempts in total |
| Broken file (cannot be decoded) | Failed at once, and any older analysis of the track is removed. Not retried while the file is unchanged |
| File changed during analysis, or differs from what the scanner recorded | Result dropped, job postponed by 30 s without counting an attempt. Network shares report modification times lazily, so a file can still be copying after it looked settled |
| Worker crashed | Its jobs are reclaimed after a 5 min lease, or immediately when the same instance restarts |
| Analyzer shut down | Running jobs are put back without counting the attempt |

Re-analysis happens automatically when `ALGORITHM_VERSION` in `heron-analysis` is bumped (algorithm or decoder changes) or when the analysis settings change (their fingerprint is stored as `params_hash`). Version 2 (M1b) adds the Chromaprint fingerprint, and version 3 (M5) the waveform overview, so each of those upgrades analyzes every track once more (in the background: tracks keep playing with their previous analysis).

**Waveform:** the analysis also keeps the highest sample peak of every 50 ms, on a dB scale (one byte per point, from −72 dBFS to full scale: quiet intros and outros stay visible), in `track_waveforms`. The web UI draws it with the cue and fade points (`GET /api/v1/tracks/{id}/waveform`), and shows the level in dBFS under the pointer and at the playhead. About 1.2 kB per minute of audio.

**Levels:** each track's lowest and highest peak in dBFS **between its cue-in and its cue-out** (its overrides first), from the waveform: the quietest and loudest 50 ms of what goes on air, its silences at both ends left out. A minimum at −72 dBFS reads "−72 dBFS or below". `heron-api` computes them from the waveform when a track is analyzed again or its cue points change (`track_levels`), within a few seconds. Levels from an earlier analysis are not shown: a track whose file changed has none until it is analyzed again.

## Analyzer

`heron-analyzer` runs `HERON_ANALYZER_WORKERS` workers in parallel, by default the number of CPUs minus one, so the engine always keeps a core on small machines such as a Raspberry Pi.

Decoding uses Symphonia:

| Format | Status |
|---|---|
| MP3 (MPEG-1/2 layers I, II, III) | Supported, including large ID3v2 tags (embedded cover art) |
| M4A / MP4: AAC-LC, ALAC | Supported |
| FLAC, Ogg Vorbis, WAV, AIFF | Supported |
| HE-AAC (AAC+) in M4A | Not supported by Symphonia's AAC decoder |
| Opus | Not supported by Symphonia; needs libopus (BSD), see the [open questions](open-questions.md) |
| MP3 inside a WAV (RIFF) container | Not supported, reported as failed |

Decoding is gapless (encoder delay and padding removed). MP3s with a broken Xing/LAME header, whose frame count makes gapless trimming drop everything, are decoded again without gapless trimming. Decoded audio peaking above +12 dBTP is rejected as corrupt: playing it would be dangerous.

## Measurements

Audio is measured in **5 ms frames** (RMS of all channels), which is the time resolution of every detected point.

| Field | How |
|---|---|
| `duration_ms` | Decoded samples |
| `integrated_lufs` | EBU R128 integrated loudness (`ebur128`) |
| `true_peak_db` | Highest true peak across channels, dBTP |
| `cue_in_ms` | First non-silent frame, ignoring blips |
| `cue_out_ms` | End of the last non-silent frame, ignoring blips, or the start of a long internal silence |
| `has_internal_silence` | A silence longer than `max_internal_silence_ms` ends the track there (hidden tracks) |
| `fade_out_start_ms` | Crossfade point, see below |
| `bpm` | Tempo, see below |

A track with no audible content has `cue_out_ms <= cue_in_ms` and is never played.

### Tempo

The tempo (BPM) is measured with the rest, on the whole track:

1. Every 10 ms, the spectral flux: how much louder each frequency got since the previous frame, on a log scale, against the loudest neighbouring frequency of that frame (SuperFlux), so steady tones count for nothing. Summed over the whole spectrum, where hi-hats dominate, and over the bass (below 200 Hz), where kick drums and bass lines mark the beat; each relative to its mean, smoothed over 50 ms.
2. The autocorrelation of that onset curve: a steady beat repeats every beat period.
3. The tempo between 60 and 200 BPM (0.05 BPM steps) whose beat and its multiples (two to four beats, counting less) repeat best, weighted towards 120 BPM, which settles most half or double tempo doubts.

Tracks shorter than 10 s, or whose onsets do not repeat (at the beat, at least 0.2 of their energy; drum patterns reach 0.75 and more), get no tempo rather than a wrong one. Generated drum patterns from 72 to 150 BPM are measured within 0.5 %; outside that range, a pattern repeating every two beats (a backbeat) can be measured at half or double tempo. Programmers correct it by hand: the tempo set by hand replaces the measured one.

### What counts as silence

A frame is silent when it is below **both**:

- the relative threshold: `start_threshold_db` (−20 dB) at the start, `end_threshold_db` (−24 dB) at the end, measured from the track's integrated loudness;
- the absolute ceiling `silence_ceiling_dbfs` (−50 dBFS).

The ceiling exists because relative thresholds alone cut real music. On a test library of about 1,800 real tracks (progressive rock, metal), thresholds relative to loudness alone trimmed more than 3 s from 171 tracks: soft intros of loud masters were taken for silence (for example 55 s of *Matmos* by Amplifier, 20 s of *Perfume River* by Fish). With the ceiling, those intros are kept.

On the full test library (4,364 tracks), 120 tracks (2.8 %) still start more than 3 s in. Some are inaudible fade-ins (the first 33 s of *Matmos* rise from −85 dBFS), but others are deliberate quiet intros between −60 and −50 dBFS: the wind of *One Of These Days* (Pink Floyd, 27 s trimmed), the nature sounds of *Close To The Edge* (Yes, 23 s). A −60 dBFS ceiling would keep most of them; see the [open questions](open-questions.md).

**Blips** (isolated sounds of `blip_max_ms` or less, 10 ms by default, such as clicks) are treated as silence.

### Crossfade point

`fade_out_start_ms` is where the next track starts:

1. Find the end of the last frame louder than `fade_trigger_db` (−15 dB from the integrated loudness) before the cue-out.
2. Keep it between `fade_max_ms` (2,500 ms) and `fade_min_ms` (150 ms) before the cue-out.

So a long quiet outro is crossfaded at most 2.5 s before its end, and an abrupt ending still gets a 150 ms overlap.

## Settings

Stored as JSON under the `analysis` name in the `settings` table; missing fields take the defaults. Changing them re-analyzes every track.

| Setting | Default |
|---|---|
| `start_threshold_db` | −20 |
| `end_threshold_db` | −24 |
| `silence_ceiling_dbfs` | −50 |
| `blip_max_ms` | 10 |
| `max_internal_silence_ms` | 15,000 |
| `fade_trigger_db` | −15 |
| `fade_min_ms` | 150 |
| `fade_max_ms` | 2,500 |

To try settings on files without touching the database:

```sh
HERON_ANALYSIS_SETTINGS='{"silence_ceiling_dbfs": -60}' \
  cargo run --release -p heron-analysis --example analyze -- path/to/*.mp3
```

## Status

`GET /api/v1/library/status` returns the track counts (present, missing, analyzed, silent), the job queue and the last scan report. The web page shows it.

Metrics: `heron_library_scans_total`, `heron_library_scan_duration_seconds`, `heron_library_files` (heron-api); `heron_analysis_jobs_total{result}`, `heron_analysis_seconds`, `heron_analysis_audio_ms_total` (heron-analyzer).
