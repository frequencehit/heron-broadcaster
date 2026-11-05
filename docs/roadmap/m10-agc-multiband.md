# M10 – AGC & multiband processing

**Depends on:** M9

## Why

A station's sound is part of its identity: an even level from one track to the next and within them, a bass that holds on small speakers, a dense sound for a hits format or an open one for talk, and no peak that distorts in the encoder. Until now Heron brings each track to the loudness target with a constant gain and hard-clips the master bus at full scale as a last safety: nothing limits the peaks, and deck EQ boosts ([M9](m9-deck-eq.md)) can clip. M10 adds the master processing chain of a broadcast automation, modeled on SAM Broadcaster's AGC tab.

## Scope

- **The chain, on each station's master bus, in this order** ([Requirements – Master AGC chain](../requirements.md#master-agc-chain)): gated AGC, stereo expander, bass EQ, 5-band processor, dual-band processor, final limiter or clipper. Design: [Audio engine – Master AGC chain](../audio-engine.md#master-agc-chain).
  - **Gated AGC:** a slow gain bringing the programme to the station's loudness target, held while the level is under the gate.
  - **Stereo expander:** widens programmes that are close to mono, above a threshold.
  - **Bass EQ:** a shelf or peak, the filters of the deck EQ.
  - **5-band and dual-band processors:** Linkwitz-Riley crossovers; per band compressor, expander or limiter mode, threshold, ratio, attack, release, hold, band gain; links between adjacent bands; overdrive into the 5 bands.
  - **Final stage:** a look-ahead limiter (5 ms of delay) or a clipper, at a ceiling.
- **Switches:** each block on or off, and bypass all. A bypassed chain is not computed: the audio is bit-identical to no chain.
- **Smoothed changes:** parameters, presets, switching blocks and the chain glide over about 50 ms, without clicks.
- **The hot handover** ([M8](m8-hot-engine-upgrade.md)) keeps the chain: settings, the AGC's gain, and the limiter's delay at the cut.
- **Presets:** save the chain under a name, load, update, rename, delete; shared by the stations. A fresh install has a few starting points.
- **Meters:** the chain's input and output levels, the AGC's gain, the gain reduction of each band and of the final stage.
- **API:** `GET`, `PUT /settings/agc` per station, presets at `/agc-presets` (operators); the `agc` live message; the chain's meters in the `levels` message.
- **Web UI:** an **AGC** page: one panel per block, each saying in a line what it does, the switches, the meters, presets.

## Out of scope

- Pre-emphasis (50 / 75 µs): it only matters for FM transmitters.
- A chain per output or per deck: one chain per station, on its master bus.
- True-peak limiting: the final stage limits sample peaks; its ceiling (−1 dBFS by default) leaves room for the encoders.
- Sidechains, ducking, processing of the FX player alone.
- The chain in the transition preview of the Crossfade & loudness page, which stays the decks' audio.

## Deliverables

- `heron-dsp`: the blocks and the chain (`crates/dsp/src/agc/`), each with test-signal tests.
- `heron-core`: `AgcSettings` with its ranges and defaults, the `agc` settings group.
- `heron-engine`: the chain on the master bus, the `SetAgc` mixer command, the `SetAgcSettings` gRPC call, `agc` in `config.json` and `GetConfig`, `heron-engine ctl set-agc`, the chain's meters in `GetLevels`, the AGC's gain and the chain's delay in the handover.
- `heron-api`: the `agc` settings group of each station pushed to its engine, the `agc_presets` table (migration, seeded), the routes, the live messages; AGC settings and presets in configuration backups.
- `heron-web`: the Processing page, translations.
- Docs: [Audio engine](../audio-engine.md#master-agc-chain), [API](../api.md#agc), [Data model](../data-model.md#settings), [Web UI](../web.md#pages), [Several stations](../stations.md#settings), [Configuration backups](../backups.md).

## Acceptance criteria

The tolerances are a starting point: the first code commits measure them and write the results here.

- [x] Each block is unit-tested with generated test signals (sines, steps, bursts, noise, silence) (`heron_dsp::agc::tests`):
  - Gated AGC: brings a sine 6 dB under or over the target to it within 0.5 dB; never beyond `max_gain_db`; holds its gain under the gate and through a pause. Its loudness meter reads the sines of EBU Tech 3341 within 0.05 LU.
  - Stereo expander: a mono signal stays mono; a near-mono signal gets the side boost its settings give, within 0.3 dB; a wide one and one under the threshold are untouched.
  - Bass EQ: the response matches the filter design within 0.1 dB from 20 Hz to 20 kHz.
  - Crossovers: the 5 and the 2 bands, flat and at 1:1, add up to the input within 0.1 dB from 20 Hz to 20 kHz; each band is −6 dB at its crossovers.
  - Band dynamics: the static curve of each mode within 0.5 dB of threshold and ratio (outside the knee); attack, release and hold times within 10 %; linked bands apply the same gain reduction; harmonic distortion of a sine compressed by 10 dB with the default times: 0.35 % at 50 Hz, 0.18 % at 100 Hz, 0.02 % at 1 kHz (tested under 0.5, 0.25 and 0.05 %).
  - Final stage: no sample over the ceiling on any test signal, among them full-scale square waves, sines 24 dB over the ceiling and bursts; under the ceiling, the limiter is a pure 5 ms delay and the clipper bit-identical.
- [x] A bypassed chain, and a chain with every block off, give bit-identical audio; each block on with neutral settings changes a signal by under 0.1 dB (`a_bypassed_chain_leaves_the_audio_as_it_is`, `each_block_with_neutral_settings_changes_little`; in the mixer, `a_bypassed_chain_reports_nothing_and_delays_nothing`).
- [x] No clicks when a parameter moves, a preset loads, or a block or the chain is switched. A 300 Hz sine goes through the change; a click is energy above 3 kHz around it more than 10 dB over the steady sound's and louder than −70 dB from the signal. Every glide and switch stays within 3 dB of the steady sound (about −80 dB from the signal), and the same changes made at once, the control, click, by 12 to 33 dB (`switching_does_not_click`, `settings_glide_without_clicks`; the M9 method of comparing steps fails on compressors, whose level moves). Checked by ear too, on a demo station's stream.
- [x] The audio thread still never allocates: `tests/no_alloc.rs` moves every block's parameters and switches them during crossfades, bypasses the chain and starts the AGC at a gain.
- [x] A control moved in the web UI is heard within 200 ms, and other open pages follow it. (The API pushes a change to the engine before it answers; on a demo station, a second open Processing page followed each arrow-key step on a slider within 10 to 45 ms, and the chain's meters showed 100 ms after the chain was switched on.)
- [x] Presets: save, load, update, rename, delete through the API and the web UI; the seeded presets are there on a fresh install (`e2e::agc_chain_and_presets`; `agc::tests` checks every seeded preset is valid and written out in full; the web UI driven in a browser on a demo station).
- [x] The chain survives an engine restart and a hot handover: the new engine plays with its settings from its first sample, at the AGC gain the old one had (within 0.5 dB), and the stream has no longer silence at the cut than without the chain (`tests/handover.rs`). An engine from before M10, started on the same state (a rollback), plays on without the chain. (`tests/handover.rs`, the AGC at −4.2 dB on both sides of the cut, 70 to 90 ms of silence in the stream as before; `config::tests`. The rollback, with the 0.2.0 engine on a demo station's state: started cold, it resumes the track where it was and leaves the `agc` settings in `config.json`; as a standby, it takes over from an M10 engine with 88 ms of silence at the cut and the listeners kept. It then saves its configuration without the chain, so an M10 engine taking over again starts bypassed until the API pushes the chain back, as it does when an engine reconnects.)
- [x] The silence watchdog still skips a silent track with the chain on and the AGC at its highest gain: silence is measured before the chain (`silence_is_measured_before_the_agc_chain`: a track at −54 dBFS lifted by 13 dB on the master bus still counts as silent).
- [x] The Processing page shows the input and output levels, the AGC's gain and each band's gain reduction, live (on a demo station with the Pop preset: linked bands 1 and 2 showed the same reduction).
- [x] No audible artifacts on a reference playlist (music of several styles, speech, a jingle over a track, a crossfade), listened to with each preset and with each block alone. (Listened to on a demo station's stream.)
- [x] CPU used by the chain measured and written in [Audio engine](../audio-engine.md#master-agc-chain): under 5 % of one core with every block on, stereo, 48 kHz. (Apple M1 Pro: 1.1 % of a core for the chain alone; about 2.8 % in a running engine, with the `Dense` preset.)

## References

- [Requirements – Master AGC chain](../requirements.md#master-agc-chain)
- [Audio engine – Master AGC chain](../audio-engine.md#master-agc-chain)
- [Audio engine – Signal quality](../audio-engine.md#signal-quality)
- [Engine handover](../handover.md)
