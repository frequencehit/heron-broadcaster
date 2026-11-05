# Audio engine

How `heron-engine`, the real-time playout process, works. Functional requirements for playout (decks, crossfading, Gap Killer, loudness) are in [Requirements](requirements.md#2-playout). Built in [M2](roadmap/m2-engine-core.md); made to stay on air in [M3](roadmap/m3-reliability.md), and through upgrades in [M8](roadmap/m8-hot-engine-upgrade.md).

## Threads

```
                  gRPC (heron-api, heron-engine ctl)
                                │
                                ▼
 decode thread A ──ring──►┌──────────┐ commands ┌──────────────┐ ring ┌─────────┐ ┌─► encoder thread (MP3 128k) ─┬─► output task (Icecast A)
 decode thread B ──ring──►│  player  │─────────►│ audio thread │─────►│ fan-out │─┤                              └─► output task (Icecast B)
 decode thread FX ─ring──►│ (control)│◄─────────│ mixer, clock │      └─────────┘ └─► encoder thread (MP3 64k) ──► output task (Icecast C)
                          └──────────┘  events  └──────────────┘                 └─► monitor (optional)
```

| Thread | Role | May allocate, lock, log |
|---|---|---|
| **Audio thread** | Renders the master bus 10 ms at a time against the clock: decks, fades, loudness gain, [deck EQ](#deck-eq), FX player, [AGC chain](#master-agc-chain) | **No** |
| Decode threads | One per loaded track: decode, resample to 48 kHz, cut at the cue points, buffer 10 s ahead | Yes |
| Preload threads | One per upcoming track: read its file into memory ahead of time | Yes |
| Player (tokio task) | Owns the queue, loads tracks, plans transitions, handles the mixer's events every 10 ms, runs the silence watchdog, saves the queue | Yes |
| Fallback scanner | Scans the fallback directory every minute and analyzes new files | Yes |
| Queue writer | Writes the saved queue to disk | Yes |
| Watchdog | Exits the process when the audio thread is stuck for 10 s, so Docker restarts it | Yes |
| Fan-out | Copies the master bus to every encoder (and the monitor) | Yes |
| Encoder threads | One per encoder profile in use | Yes |
| Output tasks | One per output: connect, stream, metadata, reconnect | Yes |

Every hop between them is bounded and **drops rather than blocks**, so nothing downstream can stall the audio thread, and one encoder or output cannot stall another.

## Format and clock

- Internal format: **f32, stereo, 48 kHz**, blocks of 480 frames (10 ms).
- **No sound card required.** The audio thread renders each block against the system's monotonic clock, 50 ms ahead of real time. Deadlines come from the number of frames rendered since the start, so timing errors never accumulate: 24 h of stream hold exactly 24 h of audio. After a stall of more than 1 s (machine suspended), the clock starts again from now instead of bursting audio, and counts a reset.
- A local monitor output is optional: build with `--features monitor` and run with `--monitor` (`HERON_MONITOR=true`). It plays the master bus on the default sound card (`cpal`), fed from the fan-out like an encoder. Docker images are built without it.

## Real-time safety

- The audio thread **never allocates, locks, does I/O or logs.** It talks to the rest of the engine through lock-free ring buffers (`rtrb`: commands in, events and finished tracks out) and atomics (positions, levels, counters). Finished tracks are handed back to the player, which drops them.
- Decode ahead into buffers. The mixer only reads audio that is ready; a cued track starts only once 100 ms of it is decoded.
- **The engine never touches the database** ([Locked decisions](decisions.md)). It receives tracks with their cue points over gRPC.
- `tests/no_alloc.rs` drives the mixer through crossfades, skips, pauses, the FX player, underruns, queue changes, EQ changes and AGC chain changes under an allocator that counts every allocation and free made on the audio thread: there must be none.

## Decoding

### Preloading

A track is decoded only 10 s ahead of its deck: without more, a library that becomes slow or unreachable (a network share) would stop the track on air within seconds. So the files of the next queued tracks (3 by default, `HERON_PRELOAD_TRACKS`) are read whole into memory, each by a thread of its own, long before they are cued. The track then decodes from memory and plays to the end even if its file is deleted or the library disappears. A file not read in time, or larger than 512 MiB, is read from disk as it plays; a file that cannot be read at all is skipped when its turn comes.

Decode threads use the analysis' own decoder (`heron_analysis::Decoder`), so the engine plays exactly the samples the analysis measured and cue points line up:

- gapless decoding, with the fallback for MP3s whose broken Xing/LAME header makes gapless trimming drop everything;
- NaN and infinite samples replaced by silence, samples beyond ±4.0 (+12 dB) clamped as corrupt data;
- resampled to 48 kHz (`heron_dsp::StreamResampler`, FFT, aligned with the input and exact in length);
- mono played on both channels; surround reduced to its front left and right;
- cut at the cue points after resampling, so positions in milliseconds mean the same as in the analysis;
- starting more than 30 s into the file (resuming after a restart), the audio before is decoded without being resampled, to get there in a fraction of the time, and resampling starts 1 s before on a frame that falls exactly on an output frame: the audio is the same as from the start.

## Transitions

The player plans each transition when it loads the next track, as soon as the current one starts ([`heron_core::playout::plan_transition`](../crates/core/src/playout.rs)). The plan is a pure function of the analysis, the per-track overrides and the settings, so it is deterministic and the UI can preview it. The audio thread then starts the next track on the exact frame planned.

1. **Where the next track starts:** at the analyzed crossfade point (`fade_out_start_ms`, mode `auto`), or a fixed time before the cue-out (mode `fixed`).
2. **Back to back instead**, the next track starting on the frame the current one ends, when either track is `min_track_ms` long or shorter (20 s: jingles, IDs), when the track disables crossfading, or when there is no usable crossfade point. A track of `min_track_ms` or shorter that **sets where the next track starts itself** (a fixed time before its cue-out, from the track's page) keeps that point: a two-second jingle can start the next track half a second before its end. Such a transition has no fades, so neither track's sound changes: they only overlap. A longer track followed by a short one stays back to back, whatever its own point.
3. **Fade-out** of the outgoing track from the moment the next starts, over `fade_out.time_ms` at most (never past the cue-out), down to `fade_out.level_pct`.
4. **Fade-in** of the incoming track over `fade_in.time_ms`, from `fade_in.level_pct`.
5. If the audio of a track ends before its cue-out (truncated file), the next track starts on that frame.

**Skip** fades the current track out over `skip_fade_ms` and starts the next one at once. **Pause** fades out over 10 ms and holds the decks; the stream keeps going with silence. Back-to-back tracks alternate between Deck A and Deck B. The **FX player** plays on top of the decks and does not change what is on air.

### Jingles over the intro

A track whose category type plays **over the intro** (on for *Jingle*, set on the *Categories* page) does not wait for its turn on a deck when the track after it has a long enough intro: the player cues that next track, and the jingle with it, to start on the FX player when the track reaches its intro point minus the jingle's length. The jingle ends where the vocals start.

- The intro point (`track_markers.intro_ms`) comes from an import from the previous software (see [Importing](import.md)) or is set on the track's page (the *Intro ends* flag, or the *Playout* card's field), on the file's timeline: the engine counts it from the track's cue-in. Queued entries of the track follow a change within seconds.
- The intro must be at least as long as the jingle (its cue-in to cue-out), and the next track must not be a jingle itself. Otherwise, or when nothing is queued after it yet, the jingle plays on its own between the tracks, as before. A jingle cued alone is cued again over the next track's intro when that track is queued in time.
- In the queue, the jingle stays before its track (marked *Over the intro*) and can be moved or removed: the pair is cued again from the new order. Inserting a track between them makes the jingle play on its own.
- Skipping the track before its jingle starts drops the jingle; skipping it while the jingle plays fades both out.
- The jingle is in the play history (on the FX deck) and counts for rotation like any pick. It sends no metadata if its type sends none.
- There is no ducking: the jingle is mixed over the intro at its own level (loudness-normalized like any track).
- A handover waits until the jingle has played.
- *On air* shows it coming: the FX player card says *Next* with a countdown to the jingle's start (in the warning color for its last 5 s), and the song's progress bar marks where the jingle starts. The engine's state gives the waiting jingle as `pending_jingle`; in the queue, before the song starts, its badge says where in the song it starts.

Every deck playing a track with an intro point also counts down to the vocals (*Vocals in 0:08.0*) until they start, and marks the end of the intro on its progress bar.

Fade curves (shape of a fade-in; a fade-out is its mirror):

| Curve | Shape |
|---|---|
| `linear` | Straight line |
| `s_curve` | Raised cosine, gentle at both ends |
| `logarithmic` | Rises fast, then levels off (a fade-out holds, then drops) |
| `exponential` | Rises slowly, then fast (a fade-out drops, then trails off) |

### Loudness

Each track gets a constant gain bringing its integrated loudness (EBU R128, from the analysis) to the target. Boosts stop at `max_gain_db`, and never push the track's true peak above `peak_ceiling_dbtp`: the gain is constant, it cannot limit. Tracks without a measured loudness play at their own level. The [AGC chain](#master-agc-chain) can then even out the level and limit the peaks; the master bus is hard-clipped at full scale as a last safety.

## Playout settings

Stored under `playout` in the `settings` table (JSON, missing fields take the defaults), pushed to the engine over gRPC. Per-track crossfade overrides (`track_overrides.crossfade`) use the same fields for one track: fade-out, mode and fixed time for the transition out of it, fade-in for the transition into it, and `"crossfade": false` to play it back to back.

| Setting | Default |
|---|---|
| `crossfade.fade_out` | enabled, `linear`, 2,500 ms, to 0 % |
| `crossfade.fade_in` | disabled (`linear`, 1,000 ms, from 0 %) |
| `crossfade.mode` | `auto` (`fixed`: `fixed_ms` before the cue-out, 2,000 ms) |
| `crossfade.min_track_ms` | 20,000: shorter tracks are never crossfaded, unless the track sets where the next one starts (then without fades) |
| `crossfade.skip_fade_ms` | 1,000 |
| `loudness.enabled` | true |
| `loudness.target_lufs` | −16 |
| `loudness.max_gain_db` | 12 |
| `loudness.peak_ceiling_dbtp` | −1 |
| `silence_watchdog.enabled` | true |
| `silence_watchdog.timeout_ms` | 20,000: longer than the analysis' longest silence inside a track (15 s) |

The crossfade point itself (dB trigger, minimum and maximum fade time) is found at import, see [Import and analysis](analysis.md#crossfade-point).

## Deck EQ

Two 10-band graphic EQs, built in [M9](roadmap/m9-deck-eq.md): **music**, on Decks A and B, and **FX**, on the FX player. Decks A and B take turns, so they share one EQ, and a track sounds the same on either.

```
deck A: source ─► × gain (loudness, fades) ─► EQ (music settings, deck A's filters) ─► meter ─┐
deck B: source ─► × gain (loudness, fades) ─► EQ (music settings, deck B's filters) ─► meter ─┼─► Σ ─► pause ─► AGC chain ─► clip ─► master meter
FX:     source ─► × gain (loudness, fades) ─► EQ (FX settings,    FX filters)       ─► meter ─┘
```

During a crossfade both decks play through the music EQ, each with its own filters. A deck's filters start from rest when a track starts on it, so nothing of the previous track rings into the next.

**Filters.** Ten filters in series, the designs of the RBJ *Audio EQ Cookbook*: a low shelf for 60 Hz, peaking bands at 170, 310, 600, 1k, 3k, 6k, 12k and 14k Hz, and a high shelf for 16 kHz (shelves with a slope of 1, Q 0.71). The shelves' midpoints are an octave inside, at 120 Hz and 8 kHz, so 60 Hz and 16 kHz are on their plateau and the curve keeps the slider's gain out to 20 Hz and 20 kHz: with the midpoints at 60 Hz and 16 kHz, meeting a slider there took twice its gain, which the curve then reached beyond (a slider at +5 dB gave +9 dB at 20 Hz). The bands are not evenly spaced (1k to 3k is 1.6 octaves, 12k to 14k a quarter of one), so each peaking band is as wide as the distances to its neighbours, on average: from Q 1.1 at 3k to Q 7 at 14k. A filter at 0 dB passes the signal through.

**Sliders and filter gains.** Neighbouring bands overlap: with each filter at its slider's gain, the curve would sag between distant bands and pile up between close ones (all sliders at +6 dB would give 4 to 14 dB). So the filter gains are solved for: those whose summed response, at each band's centre, is that band's slider (a 10 × 10 linear system, solved again four times around the gains found, as the response is not linear in the gain). The curve then meets every slider at its band, within 0.2 dB, and from 20 Hz to 20 kHz it stays within half the sliders' gain when they are all equal (all at +6 dB: 3.8 to 6 dB; at +12 dB: 7.7 to 12.1 dB), and hardly rises above the highest slider (by under 2 dB for 9 random settings in 10; a preset tops out at its highest slider). Filter gains are limited to ±24 dB: sliders that pull hard in opposite directions on close bands (12k at +12, 14k at −12, 16k at +12) need more, and meet their bands less exactly, within 3.5 dB. About one random setting in fifty does: over 1,000 random settings, 90 % meet their bands exactly and 99 % within 1 dB. Flat sliders give filters at 0 dB. The Deck EQ page draws the curve with the same computation.

The filters run as trapezoidal state-variable filters (Andrew Simper's *bell* and *shelves*), whose response is exactly the cookbook's but whose state stays valid when their gain changes as they play: a direct form biquad clicks there. They compute in `f64`: `f32` filters are noisy at 60 Hz with a 48 kHz rate. `heron_dsp::eq` holds the design, the solving and the filters; the engine only feeds them.

**Changes.** When the settings change, the player solves the filter gains and sends them to the mixer in a `SetEq` command (EQ, enabled, ten filter gains: no allocation, nothing solved on the audio thread). The audio thread moves each filter's gain towards its target with a one-pole smoother (time constant 15 ms: there in about 50 ms), recomputes the coefficients every 32 frames (0.7 ms) and interpolates them sample by sample in between, so a slider dragged from −12 to +12 dB or a preset loaded glides instead of stepping, without zipper noise. **Bypassing** glides every band to 0 dB, then stops computing the EQ: from there the deck's audio is untouched, bit for bit. **Enabling** starts from 0 dB and glides to the gains. A track starting on a deck takes the EQ's current gains, without a glide.

**Headroom.** Boosts can take a track over full scale: the master bus clips there, unless the [AGC chain](#master-agc-chain)'s final stage is on, which limits them to its ceiling. The loudness gain does not account for the EQ. Cut what is too much rather than boosting everything else.

**Settings** (`eq`, see [API – Deck EQ](api.md#deck-eq)), kept apart from the playout settings so the sliders can move often without sending them:

```json
{"music": {"enabled": true, "gains_db": [3, 2, 0, 0, 0, 0, 0, 1, 1, 0]},
 "fx":    {"enabled": false, "gains_db": [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]}}
```

Ten slider gains, from 60 Hz up, each from −12 to +12 dB. By default both EQs are flat and bypassed. The engine receives them over gRPC (`SetEqSettings`) and keeps them in `config.json` under `eq`: it restarts with them, and a new engine taking over ([Upgrades and handover](#upgrades-and-handover)) loads them before its first sample. Engines from before M9 ignore the key.

## Master AGC chain

The processing of a station's sound, built in [M10](roadmap/m10-agc-multiband.md) and modeled on SAM Broadcaster's AGC tab. One chain per engine, so per station, on the master bus: after the decks and the FX player are summed and the pause applied, before the clip at full scale, the master meter and the encoders.

```
Σ ─► pause ─┬─► gated AGC ─► stereo expander ─► bass EQ ─► 5-band ─► dual-band ─► limiter or clipper ─► clip ─► master meter ─► encoders
            └─► silence detection, chain input meter
```

What each block is for:

| Block | Does | Typical use |
|---|---|---|
| **Gated AGC** | Rides a slow gain to bring the programme to the station's loudness target; holds it while the level is under the gate, so silences, fades and noise are not pumped up | Tracks without a measured loudness, quiet and loud passages, the voice of a live DJ ([M11](roadmap/m11-sinks-live-dj.md)) |
| **Stereo expander** | Widens programmes that are close to mono | Old recordings, mono jingles |
| **Bass EQ** | One shelf or peak in the bass | A fuller bass, or less of it |
| **5-band processor** | Splits the sound into five bands, each with its own compressor, expander or limiter and gain | Density, and a consistent tonal balance from one track to the next |
| **Dual-band processor** | The same in two bands, low and high | A final density stage, or a simpler processor on its own |
| **Final stage** | Keeps the peaks under a ceiling: a look-ahead limiter, clean, or a clipper, louder | Always, when the chain is on: no peak distorts in the encoders |

### Blocks

**Gated AGC.** Measures the programme's short-term loudness (K-weighted, over 3 s, as the EBU R128 short-term loudness), every 100 ms. Its gain moves towards the difference between the station's loudness target (`loudness.target_lufs` of the [playout settings](#playout-settings), −16 LUFS by default) and that loudness, within ±`max_gain_db`: down with a time constant of 1 s, up with one of 4 s. Tracks the loudness gain already brought to the target pass at about 0 dB. While the momentary loudness (400 ms) is under `gate_lufs`, the gain holds: a pause, a silence or the end of a fade stops it at once, where the 3 s loudness would still be falling. It starts at 0 dB on a cold start, and from the old engine's gain after a [hot handover](#upgrades-and-handover).

**Stereo expander.** Splits the stereo into mid (L + R) and side (L − R). The width is the side level against the mid level, over 300 ms. The side is boosted by `depth` × (−6 dB − width), between 0 and `level` × 6 dB: a recording whose side is already within 6 dB of its mid is left as it is, a near-mono one gets up to `level`'s boost, and a mono one stays mono (its side is silent). Under `threshold_db` (the programme's level, over 300 ms) nothing is boosted. The side below 150 Hz is never boosted: the bass stays centred, which encoders and mono listeners need.

**Bass EQ.** One filter of the [deck EQ](#deck-eq)'s kind (state-variable, `f64`): a low shelf (slope 1) or a peak (Q 1, about an octave and a third wide) at `frequency_hz`.

**5-band and dual-band processors.** Crossovers split the sound into bands; each band goes through its dynamics, then its gain; the bands add up again. The crossovers are **Linkwitz-Riley of the 4th order** (two 2nd-order Butterworth filters in series), and each band also goes through the all-pass of the crossovers that do not split it, so the bands of a flat processor add up to the input's magnitude exactly: a phase shift, no change of level. `overdrive_db` raises the level going into the five bands: more of the sound is over the thresholds, and denser; the final stage catches the peaks. A **link** between two adjacent bands makes both apply the deeper of their two gain reductions, so a strong kick does not make the low mids breathe on their own. Changing the links glides each band's gain from where it is to the new links' over 50 ms, also when they change again during a glide.

**Band dynamics.** Per band:

| Mode | Over the threshold | Under the threshold |
|---|---|---|
| `compressor` | Rises by 1 dB for each `ratio` dB | Unchanged |
| `expander` | Unchanged | Falls by `ratio` dB for each dB the input falls (at most 40 dB down): lowers noise and room sound between notes |
| `limiter` | Never rises (infinite ratio) | Unchanged |

The level is the higher peak of the two channels, so the stereo image never shifts. The gain computer has a soft knee 6 dB wide (none for the limiter); its gain, in dB, follows with `attack_ms` when it falls and `release_ms` when it rises, after holding `hold_ms` (time constants: 63 % of the way in that time). `gain_db` comes after the dynamics, to make up for them or to shape the balance.

**Final stage.** `mode`:

- `limiter`: a look-ahead limiter. The gain each sample needs to stay under the ceiling is known 5 ms ahead; the gain applied is the lowest needed over the next 5 ms, smoothed over 5 ms, so it reaches each peak's gain exactly in time and moves without steps. The audio is delayed by those 5 ms. The gain comes back with `release_ms`.
- `clipper`: no delay; clips at the ceiling. Louder and denser, and it distorts when driven hard.

Both are followed by a hard clip at the ceiling, which only catches rounding. The ceiling is on sample peaks: the encoders can make inter-sample peaks up to about a decibel higher, hence −1 dBFS by default.

### Chain settings

`agc` settings, one group per station ([API – AGC](api.md#agc)):

| Block | Settings: range (default) |
|---|---|
| — | `bypass_all` (true) |
| `gated_agc` | `enabled` (false); `gate_lufs`: −70 to −20 (−45); `max_gain_db`: 0 to 18 (6) |
| `stereo_expander` | `enabled` (false); `level`: 0 to 100 % (50); `depth`: 0 to 100 % (50); `threshold_db`: −60 to 0 dBFS (−40) |
| `bass_eq` | `enabled` (false); `shape`: `shelf` or `peak` (`shelf`); `frequency_hz`: 40 to 250 (100); `gain_db`: −12 to +12 (0) |
| `five_band` | `enabled` (false); `overdrive_db`: 0 to 12 (0); `crossovers_hz`: four, rising, at least a third of an octave apart, 40 to 16,000 (100, 400, 1,600, 6,400); `links`: four (all false); `bands`: five band settings |
| `dual_band` | `enabled` (false); `crossover_hz`: 60 to 2,000 (200); `link` (false); `bands`: two band settings |
| Band settings | `mode`: `compressor`, `expander` or `limiter` (`compressor`); `threshold_db`: −60 to 0 (−20); `ratio`: 1 to 20 (3); `attack_ms`: 0.5 to 200 (20); `release_ms`: 10 to 2,000 (250); `hold_ms`: 0 to 500 (0); `gain_db`: −12 to +12 (0) |
| `final_stage` | `enabled` (true); `mode`: `limiter` or `clipper` (`limiter`); `ceiling_db`: −6 to 0 dBFS (−1); `release_ms`: 10 to 1,000 (100) |

By default the chain is bypassed, so a new or upgraded station sounds the same as before; turned on as it is, it is the final limiter alone. The engine receives the settings over gRPC (`SetAgcSettings`) and keeps them in `config.json` under `agc`: it restarts with them, and a new engine taking over loads them before its first sample. Engines from before M10 ignore the key, and play on without the chain. One that takes over from a newer engine (a rollback) saves its configuration without it: when a newer engine takes over again, the chain stays bypassed until the API pushes it back, as it does whenever an engine reconnects.

### Switching and changes

- **Bypass all** skips the chain: it is not computed, and the audio is bit-identical to an engine without it. So is a chain with every block off. A block that is off is not computed either.
- **Switching** a block, or the whole chain, crossfades between the audio with and without it over 50 ms. A block switched on starts from rest. Switching the limiter crossfades its 5 ms delay too: a brief phasing, no click.
- **Parameters** glide to new values in about 50 ms (a one-pole smoother, 15 ms time constant, as the [deck EQ](#deck-eq)): gains, thresholds, ratios, times, crossover frequencies (in octaves). Filters are designed again at every sample while they glide: in steps, they would whine at the rate of the steps. After a change of mode, a band's gain moves to its new value with its attack or release, as it does when the level changes. Lowering the ceiling is followed by the limiter's look-ahead: the audio already delayed keeps the ceiling it was limited for.
- **Pause:** the chain gets silence; the AGC, under its gate, holds its gain until the music comes back.

### Implementation

- `heron_dsp::agc` holds the blocks and the chain; the engine only feeds them. All their state (filters, envelopes, the 3 s loudness window, the look-ahead line) is allocated when the engine starts. The player checks and converts the settings and sends them to the mixer in a `SetAgc` command of plain numbers, as `SetEq` does: nothing is allocated or computed from the settings on the audio thread but the filters' coefficients.
- Filters compute in `f64` (crossovers and EQ at low frequencies are noisy in `f32`), the rest in `f32`.
- **Latency:** 5 ms while the limiter is heard (on its own or crossfading with the clipper), none with the clipper alone or the stage off. Deck positions are counted before the chain, the master meter after it; 5 ms is not heard as a delay.
- **Meters:** with the levels, the chain reports its input and output peak and RMS, the AGC's gain, and the gain reduction of each of the 5 and the 2 bands and of the final stage. Gain reductions hold their deepest value and come back by 20 dB/s, as the peak meters.
- **CPU:** the chain with every block on (the heaviest test settings) processes 60 s of stereo at 48 kHz in 0.68 s on an Apple M1 Pro core: 1.1 % of it (`cargo test -p heron-dsp --release -- --ignored --nocapture agc::tests::cpu`). In a running engine playing a station, with the `Dense` preset (every block on), the engine used 4.4 to 4.7 % of that core against 1.7 to 1.9 % bypassed: about 2.8 % for the chain, more than it takes alone. Bypassed, it costs nothing.
- **Distortion:** a sine compressed by 10 dB in one band, with the default times (attack 20 ms, release 250 ms), comes out with 0.35 % of harmonic distortion at 50 Hz, 0.18 % at 100 Hz and 0.02 % at 1 kHz.

## Control interface

gRPC service `heron.engine.v1.Engine` ([engine.proto](../crates/proto/proto/heron/engine/v1/engine.proto)), on `HERON_GRPC_ADDR` (port 50051, inside the Docker network only): queue (enqueue, remove, clear, move, replace), skip, pause, resume, FX player, state, levels, a stream of track changes, playout settings, outputs, the deck EQ and the AGC chain. Tracks carry everything needed to play them: library path, tags, cue points, crossfade point, loudness, overrides, and the reason they play (rotation, manual…), carried back in the track changes for the play history. Queue positions count the waiting tracks: 0 is before the cued track.

The engine keeps its state in `HERON_STATE_DIR`, so it restarts streaming even when the API and the database are down:

| File | Contents | Written |
|---|---|---|
| `config.json` | Playout settings, outputs, the [deck EQ](#deck-eq) and the [AGC chain](#master-agc-chain) | When the API (or `ctl`) changes them |
| `queue.json` | The track on air and its position, the cued track and the queue, pause, the next entry ID | When the queue changes, every 5 s while playing, and on shutdown |
| `fallback.json` | Analyses of the fallback directory's files | After each analysis |
| `handover.sock` | Unix socket of the active engine, for a new engine to take over ([Upgrades and handover](#upgrades-and-handover)) | Bound while the engine is active |

Files are replaced atomically (written aside, synced, renamed). On start, the engine resumes the track on air where it was (fading in over 500 ms; a track less than 2 s from its end is not resumed), then plays the saved queue. Entry IDs are kept and new ones go on from the saved next ID, so the API's history and queue stay consistent. The resumed track's start is reported as `resumed` (`TrackStarted.resumed`): it is the same play going on, not a new one. Tracks from the fallback directory are not saved. A clean stop (`docker compose restart`) saves the exact position; after a crash, the track resumes from the last periodic save, up to 5 s earlier.

`heron-api` feeds the engine and keeps its configuration ([API and control plane](api.md)). `heron-engine ctl` is a command-line client, to inspect the engine or drive it without the API (the soak test does):

```sh
docker compose exec heron-engine heron-engine ctl state
docker compose exec heron-engine heron-engine ctl watch
docker compose exec heron-engine heron-engine ctl skip
```

`ctl enqueue` and `ctl autofill` analyze files themselves to send their cue points (`ctl enqueue --no-analysis` plays a file whole, as the API does with tracks not analyzed yet). With the API running, outputs and settings set with `ctl set-outputs`, `ctl set-settings`, `ctl set-eq` and `ctl set-agc` are replaced by the API's within a minute.

## Upgrades and handover

An engine replaced the usual way (stop, then start) is off air for a few seconds, and Icecast disconnects every listener when its source leaves. To upgrade without that, a new engine takes over from the running one: the **hot handover**, built in [M8](roadmap/m8-hot-engine-upgrade.md). The procedures that use it, for Docker Compose and Docker Swarm, are in [Upgrading](upgrading.md); its design, in [Engine handover](handover.md).

The running engine (the **active** one) listens on a unix socket in its state directory, `$HERON_STATE_DIR/handover.sock`. Both engines mount the same state volume, so they run on the same host.

### Standby

On start, the engine connects to `handover.sock`:

| Answer | The new engine |
|---|---|
| None (no socket, or nobody listening) | Starts as usual (a **cold start**): resumes `queue.json` and connects its outputs. |
| A hello with a compatible protocol version | Becomes the **standby**: it does not play, opens no output and writes no state file. `/healthz` succeeds with `"role": "standby"`, so Docker and Swarm consider it started. It does not serve gRPC yet: calls from `heron-api` that reach it (during the overlap, the `heron-engine` name points to both engines) fail as unavailable and are retried. |
| A hello with an incompatible version | Waits for the active engine to stop (the socket to disappear), then makes a cold start. |

A standby whose active engine stops without handing over (it crashes, or `handover.sock` closes) makes a cold start.

### The handover

It starts when the active engine receives SIGTERM with a standby connected. Without a standby, the engine stops as before (the queue is saved, then the process exits).

1. **Prepare.** The active engine sends its playout settings, its outputs, the queue (as in `queue.json`) and the **stop**: the position in the track on air where the new engine takes over, about 2 s ahead. The stop never falls in a transition or under an FX jingle, nor less than 1 s before the next transition: the active engine waits for a moment like that first (within the 30 s stop grace period).
2. **Ready.** The standby opens the track on air at the stop (the fast skip used to resume after a restart), queues the waiting tracks and answers `ready` once enough audio is decoded. An answer later than 300 ms before the stop is too late: the active engine prepares again with a stop three times further ahead (6 s, then 18 s, never more than the time left of the 25 s a handover may take), and gives up after three attempts.
3. **Cut.** The active engine stops the track on air on the exact frame of the stop, fading out over the last 10 ms, flushes its encoders to its outputs, then sends the open connections of its outputs (their file descriptors, passed over the socket). They are connections to `heron-relay`, which holds the TCP connections to the stream servers: a TCP connection would not outlive the old engine's container ([Engine handover – Why the connections go through a relay](handover.md#why-the-connections-go-through-a-relay)). With the [AGC chain](#master-agc-chain) on, the encoders get the chain's delay after the stop too, so nothing of the fade-out is lost, and the cut carries the AGC's gain. The new engine plays from that frame with a 10 ms fade-in, at that gain, and streams to the same connections, without connecting again. Icecast never sees the source leave; the listeners hear about 80 ms of silence.
4. **Active.** The old engine writes no state any more and exits. The new engine takes `handover.sock` (bound aside, then renamed over it), writes `queue.json`, and serves gRPC; the API's event stream reconnects to it, as after a restart.

The new engine keeps the entry IDs and the position of the track on air, and reports its start as `resumed`, as after a restart, so the play history has one entry for it and webhooks do not send it again. Its encoders start fresh: an MP3 stream continues frame after frame, and an Ogg stream (Opus, Vorbis) continues as a new chained stream. Outputs that are not a connection (HLS, file recording, pipes, from [M11](roadmap/m11-sinks-live-dj.md)) are restarted by the new engine. See [Encoders and outputs – Isolation and reliability](outputs.md#isolation-and-reliability).

| Failure during a handover | What happens |
|---|---|
| The standby dies, or is not ready after three attempts | The active engine stops as before; the standby, if alive, makes a cold start once the active engine is gone. |
| The active engine dies before passing its connections | The standby makes a cold start, from the queue saved last (up to 5 s earlier). |
| The active engine dies after passing its connections | The new engine carries on: it has everything it needs. |
| An output's connection is not passed (closed, or not a connection) | The new engine connects that output again, as after a restart. |

## Signal quality

- Gain changes are ramps (fades, skip, pause), never steps, to avoid clicks.
- Parameter changes of later DSP are **smoothed** too: the [deck EQ](#deck-eq) and the [AGC chain](#master-agc-chain) glide to new settings in about 50 ms.

## Failure handling

Every failure path ends with audio continuing, in this order:

1. next track;
2. fallback directory;
3. generated silence filler, as the very last resort (with an alert).

| Failure | What happens |
|---|---|
| A queued file is missing, unreadable or corrupt | Read ahead into memory when it is queued (it plays even if deleted later); if it cannot be read or decoded, it is skipped before it starts. A file that fails mid-way ends there and the next track starts. |
| The library is slow or unreachable | The next tracks are already in memory ([Preloading](#preloading)). A cued track whose file hangs (not preloaded in time) is given up after 10 s with nothing on air, and a fallback track plays instead; it is not put back in the queue. |
| The queue runs dry (the API is down or cannot reach the engine, autofill is off, restrictions forbid every track) | A track from the **fallback directory** is cued, 30 s before the track on air ends or at once when nothing plays, so it starts with the planned transition. Anything queued replaces a cued fallback track. |
| A track stays silent | The **silence watchdog** skips it after `silence_watchdog.timeout_ms` of silence (below −50 dBFS) on the master bus, measured before the [AGC chain](#master-agc-chain) so its gain never hides one. If the next track is silent too, it skips to the fallback directory instead. Pauses do not count. |
| Nothing to play at all | Silence is streamed (the outputs stay connected). After 5 s, the engine logs an error, sets `heron_engine_dead_air` and warns in `/healthz`. |
| A stream server is down or restarted | The output reconnects with exponential backoff; the others are not affected. |
| The audio thread is stuck | The process exits after 10 s; Docker restarts it and it resumes its saved queue. |
| The engine restarts | It resumes the track on air and the queue ([Control interface](#control-interface)). Listeners stay connected through an Icecast fallback mount ([Upgrading](upgrading.md#icecast-fallback-mount)). |
| The engine is upgraded | The new engine takes over the queue and the open connections from the old one, without a gap ([Upgrades and handover](#upgrades-and-handover)). |

The fallback directory (`HERON_FALLBACK_DIR`, mounted read-only from `./data/fallback`) holds a few tracks that suit any hour: station IDs and evergreen music. Its files are scanned every minute (sub-folders too) and analyzed once in the background, like library tracks at import, so they play with their cue points, crossfades and loudness; the analyses are kept in `fallback.json`. A file not analyzed yet plays whole at its own level; silent and undecodable files are never picked. Every file plays once, in random order, before any plays again.

The chaos tests (`scripts/chaos-test.sh`) check each of these on a running stack. See [Requirements – Reliability](requirements.md#5-reliability-247).

## Observability

`/healthz` reports the engine's `role` (`active`, or `standby` while waiting to take over), fails when the audio thread of an active engine stops rendering, and lists `warnings` when the server clock is not synchronized (or off by more than about 1 s) or there is nothing to play. `/metrics`:

| Metric | Meaning |
|---|---|
| `heron_engine_rendered_seconds` | Audio rendered since the start |
| `heron_engine_clock_drift_seconds` | Audio rendered minus time elapsed: constant (the 50 ms lead) while the clock keeps time |
| `heron_engine_clock_max_late_seconds` | Worst lateness of a block over the last second |
| `heron_engine_clock_resets_total` | Stalls longer than 1 s |
| `heron_engine_underruns_total` | Blocks where a deck had no decoded audio |
| `heron_engine_dropped_blocks_total` | Blocks the encoders did not take in time |
| `heron_engine_silent_seconds` | How long the master bus has been silent (below −50 dBFS, before the AGC chain) |
| `heron_engine_dead_air` | 1 when there is nothing to play |
| `heron_engine_fallback_tracks_total`, `heron_engine_silence_skips_total` | Tracks started from the fallback directory, tracks skipped by the silence watchdog |
| `heron_engine_preload_failures_total`, `heron_engine_stalled_tracks_total` | Queued tracks that could not be read ahead of time, cued tracks given up because they could not start |
| `heron_clock_synchronized`, `heron_clock_offset_seconds`, `heron_clock_estimated_error_seconds` | The server clock's synchronization, from the kernel (read-only `adjtimex`) |
| `heron_output_connected`, `heron_output_bytes_total`, `heron_output_reconnects_total`, `heron_output_dropped_chunks_total` | Per output |
| `heron_encoder_dropped_chunks_total` | Per encoder profile |

Dashboards, alerts and the full list: [Monitoring](monitoring.md).

## Testability

- Crossfades are rendered offline by the same mixer code and compared, sample by sample, with references computed independently (`crates/engine/src/mixer/tests.rs`).
- The player is tested against real files with the mixer rendered offline.
- The player is tested with the fallback directory, the silence watchdog, and a restart resuming the saved queue.
- The handover is tested with two engines on one state directory and a fake Icecast server: one source connection throughout, the same queue and position after the cut, and every failure case ending in a cold start.
- `scripts/chaos-test.sh` breaks a running stack (database, API, Icecast, deleted files, a silent file, engine restarts, an engine upgrade through `scripts/upgrade.sh`) while listening to the stream.
- `scripts/soak-test.sh` plays the library to two Icecast servers for a given time and checks the clock kept time and nothing was lost; `scripts/check-streaming.sh` checks a Compose stack goes on air through the API (run in CI on amd64 and arm64).
