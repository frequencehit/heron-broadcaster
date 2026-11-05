// SPDX-License-Identifier: AGPL-3.0-or-later

//! Offline renders of the mixer compared with references computed
//! independently, sample by sample.

use std::f32::consts::{PI, TAU};

use heron_dsp::GraphicEq;
use heron_dsp::agc::ChainParams;

use super::*;

/// Largest difference allowed between a render and its reference.
const TOLERANCE: f32 = 1e-5;

/// A stereo tone; the right channel is the left one at half level, so a
/// channel swap would be caught.
fn tone(frames: usize, freq: f32, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let value = amplitude * (TAU * freq * n as f32 / SAMPLE_RATE as f32).sin();
            [value, value * 0.5]
        })
        .collect()
}

fn track(entry_id: u64, samples: &[f32]) -> Box<LoadedTrack> {
    Box::new(LoadedTrack {
        entry_id,
        length: (samples.len() / CHANNELS) as u64,
        start: 0,
        source: Box::new(VecSource::new(samples.to_vec())),
        gain: 1.0,
        fade_in: None,
    })
}

fn render(mixer: &mut Mixer, frames: usize, block: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames * CHANNELS];
    for chunk in output.chunks_mut(block * CHANNELS) {
        mixer.render(chunk);
    }
    output
}

fn events(handle: &mut MixerHandle) -> Vec<Event> {
    std::iter::from_fn(|| handle.events.pop().ok()).collect()
}

fn cue(handle: &mut MixerHandle, track: Box<LoadedTrack>, transition: FrameTransition) {
    assert!(
        handle
            .commands
            .push(Command::Cue { track, transition })
            .is_ok()
    );
}

fn assert_matches(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() <= TOLERANCE,
            "sample {index} (frame {}): rendered {a}, expected {e}",
            index / CHANNELS
        );
    }
}

#[test]
fn back_to_back_tracks_are_gapless() {
    let first = tone(10_007, 440.0, 0.5);
    let second = tone(7_919, 660.0, 0.4);
    let third = tone(5_003, 880.0, 0.3);
    // Odd block sizes: transitions fall in the middle of blocks.
    for block in [480, 333, 1] {
        let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
        cue(
            &mut handle,
            track(1, &first),
            FrameTransition::back_to_back(),
        );
        let mut output = render(&mut mixer, 5_000, block);
        cue(
            &mut handle,
            track(2, &second),
            FrameTransition::back_to_back(),
        );
        output.extend(render(&mut mixer, 10_000, block));
        cue(
            &mut handle,
            track(3, &third),
            FrameTransition::back_to_back(),
        );
        output.extend(render(&mut mixer, 10_000, block));

        let mut expected = [first.clone(), second.clone(), third.clone()].concat();
        expected.resize(output.len(), 0.0);
        assert_matches(&output, &expected);
    }
}

#[test]
fn crossfade_matches_the_reference() {
    let outgoing = tone(144_000, 440.0, 0.5); // 3 s
    let incoming = tone(96_000, 660.0, 0.5); // 2 s
    let start = 96_000_u64; // the next track starts at 2 s
    let fade_out = FrameRamp {
        start,
        length: 24_000,
        curve: FadeCurve::Linear,
        level: 0.0,
    };
    let fade_in = FrameRamp {
        start: 0,
        length: 12_000,
        curve: FadeCurve::SCurve,
        level: 0.2,
    };

    let mut reference = Vec::with_capacity(240_000 * CHANNELS);
    for frame in 0..240_000_u64 {
        let (out_gain, in_gain) = if frame < start {
            (1.0, 0.0)
        } else {
            let since = (frame - start) as f32;
            let progress = (since / 12_000.0).min(1.0);
            (
                (1.0 - since / 24_000.0).max(0.0),
                0.2 + 0.8 * (0.5 - 0.5 * (PI * progress).cos()),
            )
        };
        for channel in 0..CHANNELS {
            let sample = |audio: &[f32], frame: u64| {
                audio
                    .get(frame as usize * CHANNELS + channel)
                    .copied()
                    .unwrap_or(0.0)
            };
            let a = sample(&outgoing, frame);
            let b = frame
                .checked_sub(start)
                .map_or(0.0, |n| sample(&incoming, n));
            reference.push(a * out_gain + b * in_gain);
        }
    }

    for block in [480, 1_000, 7] {
        let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
        cue(
            &mut handle,
            track(1, &outgoing),
            FrameTransition::back_to_back(),
        );
        let mut output = render(&mut mixer, 1_000, block);
        let mut next = track(2, &incoming);
        next.fade_in = Some(fade_in);
        cue(
            &mut handle,
            next,
            FrameTransition {
                next_start: start,
                fade_out: Some(fade_out),
            },
        );
        output.extend(render(&mut mixer, 239_000, block));
        assert_matches(&output, &reference);

        let events = events(&mut handle);
        assert_eq!(
            events,
            [
                Event::Started {
                    deck: Deck::A,
                    entry_id: 1,
                    at_frame: 0
                },
                Event::Started {
                    deck: Deck::B,
                    entry_id: 2,
                    at_frame: start
                },
                Event::Ended {
                    deck: Deck::A,
                    entry_id: 1,
                    played_frames: 144_000,
                    at_frame: 144_000,
                    reason: EndReason::Finished
                },
                Event::Ended {
                    deck: Deck::B,
                    entry_id: 2,
                    played_frames: 96_000,
                    at_frame: 192_000,
                    reason: EndReason::Finished
                },
            ]
        );
    }
}

#[test]
fn planned_transition_lines_up_with_the_cue_points() {
    use heron_core::playout::{CrossfadeSettings, plan_transition};

    let settings = CrossfadeSettings::default();
    let outgoing = TrackPlayout {
        duration_ms: 30_000,
        cue_in_ms: 1_000,
        cue_out_ms: 29_000,
        mix_point_ms: Some(27_500),
        integrated_lufs: None,
        true_peak_db: None,
        crossfade: Default::default(),
    };
    let incoming = TrackPlayout {
        mix_point_ms: None,
        ..outgoing
    };
    let transition =
        FrameTransition::new(&plan_transition(&outgoing, &incoming, &settings), &outgoing);
    // The deck starts at the cue-in, so the mix point is 26.5 s into it.
    assert_eq!(transition.next_start, 26_500 * 48);
    let fade_out = transition.fade_out.unwrap();
    assert_eq!((fade_out.start, fade_out.length), (26_500 * 48, 1_500 * 48));
}

#[test]
fn applies_the_loudness_gain() {
    let samples = tone(4_800, 440.0, 0.5);
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    let mut loud = track(1, &samples);
    loud.gain = 0.25;
    cue(&mut handle, loud, FrameTransition::back_to_back());
    let output = render(&mut mixer, 4_800, BLOCK_FRAMES);
    let expected: Vec<f32> = samples.iter().map(|s| s * 0.25).collect();
    assert_matches(&output, &expected);
}

#[test]
fn skip_fades_out_and_starts_the_next_track_at_once() {
    let first = tone(96_000, 440.0, 0.5);
    let second = tone(96_000, 660.0, 0.5);
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        track(1, &first),
        FrameTransition::back_to_back(),
    );
    let mut output = render(&mut mixer, 9_600, BLOCK_FRAMES);
    cue(
        &mut handle,
        track(2, &second),
        FrameTransition::back_to_back(),
    );
    assert!(
        handle
            .commands
            .push(Command::Skip {
                fade_frames: 4_800,
                curve: FadeCurve::Linear
            })
            .is_ok()
    );
    output.extend(render(&mut mixer, 9_600, BLOCK_FRAMES));

    let mut expected = Vec::new();
    for frame in 0..19_200_usize {
        for channel in 0..CHANNELS {
            let a = first[frame * CHANNELS + channel];
            expected.push(if frame < 9_600 {
                a
            } else {
                let since = frame - 9_600;
                let out_gain = (1.0 - since as f32 / 4_800.0).max(0.0);
                a * out_gain + second[since * CHANNELS + channel]
            });
        }
    }
    assert_matches(&output, &expected);
    assert!(events(&mut handle).contains(&Event::Ended {
        deck: Deck::A,
        entry_id: 1,
        played_frames: 14_400,
        at_frame: 14_400,
        reason: EndReason::Skipped,
    }));
}

#[test]
fn pause_fades_out_and_holds_the_position() {
    let samples = vec![0.5; 96_000 * CHANNELS];
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        track(1, &samples),
        FrameTransition::back_to_back(),
    );
    render(&mut mixer, 4_800, BLOCK_FRAMES);
    assert!(handle.commands.push(Command::Pause).is_ok());
    let paused = render(&mut mixer, 4_800, BLOCK_FRAMES);
    // A 10 ms fade, then silence.
    assert!(paused[0] > 0.49);
    assert!(paused[..480 * CHANNELS].windows(2).all(|w| w[1] <= w[0]));
    assert!(paused[480 * CHANNELS..].iter().all(|&s| s == 0.0));
    let position = handle.status.decks[0].position.load(Ordering::Relaxed);
    render(&mut mixer, 4_800, BLOCK_FRAMES);
    assert_eq!(
        handle.status.decks[0].position.load(Ordering::Relaxed),
        position
    );
    assert!(handle.status.paused.load(Ordering::Relaxed));

    assert!(handle.commands.push(Command::Resume).is_ok());
    let resumed = render(&mut mixer, 960, BLOCK_FRAMES);
    assert!(resumed[0] < 0.01);
    assert!((resumed[resumed.len() - 1] - 0.5).abs() < 1e-6);
}

#[test]
fn fx_plays_on_top_of_the_decks() {
    let music = tone(9_600, 440.0, 0.3);
    let jingle = tone(2_400, 1_000.0, 0.3);
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        track(1, &music),
        FrameTransition::back_to_back(),
    );
    assert!(
        handle
            .commands
            .push(Command::PlayFx(track(2, &jingle)))
            .is_ok()
    );
    let output = render(&mut mixer, 9_600, BLOCK_FRAMES);
    let expected: Vec<f32> = music
        .iter()
        .enumerate()
        .map(|(index, sample)| sample + jingle.get(index).copied().unwrap_or(0.0))
        .collect();
    assert_matches(&output, &expected);
    assert!(events(&mut handle).contains(&Event::Started {
        deck: Deck::Fx,
        entry_id: 2,
        at_frame: 0
    }));
}

#[test]
fn track_ending_before_its_cue_out_hands_over_at_once() {
    let short = tone(1_000, 440.0, 0.5);
    let next = tone(1_000, 660.0, 0.5);
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    let mut truncated = track(1, &short);
    truncated.length = 50_000; // the analysis expected more audio
    cue(&mut handle, truncated, FrameTransition::back_to_back());
    render(&mut mixer, 10, 10);
    cue(
        &mut handle,
        track(2, &next),
        FrameTransition::back_to_back(),
    );
    let output = render(&mut mixer, 1_990, 480);
    let mut expected = [&short[20..], &next[..]].concat();
    expected.resize(output.len(), 0.0);
    assert_matches(&output, &expected);
}

/// A source whose decoder has not caught up yet.
struct Starved;

impl AudioSource for Starved {
    fn read(&mut self, _out: &mut [f32]) -> usize {
        0
    }

    fn is_finished(&self) -> bool {
        false
    }
}

#[test]
fn underruns_play_silence_and_are_reported() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        Box::new(LoadedTrack {
            entry_id: 7,
            source: Box::new(Starved),
            length: 48_000,
            start: 0,
            gain: 1.0,
            fade_in: None,
        }),
        FrameTransition::back_to_back(),
    );
    let output = render(&mut mixer, 4_800, BLOCK_FRAMES);
    assert!(output.iter().all(|&s| s == 0.0));
    assert_eq!(handle.status.decks[0].position.load(Ordering::Relaxed), 0);
    assert!(handle.status.underruns.load(Ordering::Relaxed) > 0);
    let underruns = events(&mut handle)
        .into_iter()
        .filter(|event| matches!(event, Event::Underrun { entry_id: 7, .. }))
        .count();
    assert_eq!(underruns, 1);
}

#[test]
fn output_never_exceeds_full_scale() {
    let hot = vec![3.0_f32; 960 * CHANNELS];
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(&mut handle, track(1, &hot), FrameTransition::back_to_back());
    let output = render(&mut mixer, 960, BLOCK_FRAMES);
    assert!(output.iter().all(|&s| s == 1.0));
    let (peak, _) = handle.status.master.read();
    assert_eq!(peak, [1.0, 1.0]);
}

#[test]
fn finished_tracks_are_handed_back_to_be_dropped() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        track(1, &tone(100, 440.0, 0.5)),
        FrameTransition::back_to_back(),
    );
    cue(
        &mut handle,
        track(2, &tone(100, 440.0, 0.5)),
        FrameTransition::back_to_back(),
    );
    render(&mut mixer, 480, BLOCK_FRAMES);
    // Track 1 was replaced while cued, track 2 played to the end.
    let returned: Vec<u64> = std::iter::from_fn(|| handle.trash.pop().ok())
        .map(|track| track.entry_id)
        .collect();
    assert_eq!(returned, [1, 2]);
    assert_eq!(events(&mut handle)[0], Event::Uncued { entry_id: 1 });
}

/// A decoder that has not produced anything yet.
struct NotReady;

impl AudioSource for NotReady {
    fn read(&mut self, _out: &mut [f32]) -> usize {
        0
    }

    fn is_finished(&self) -> bool {
        false
    }

    fn is_ready(&self) -> bool {
        false
    }
}

#[test]
fn cued_tracks_wait_for_their_audio() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        Box::new(LoadedTrack {
            entry_id: 1,
            source: Box::new(NotReady),
            length: 48_000,
            start: 0,
            gain: 1.0,
            fade_in: None,
        }),
        FrameTransition::back_to_back(),
    );
    render(&mut mixer, 4_800, BLOCK_FRAMES);
    assert_eq!(handle.status.on_air.load(Ordering::Relaxed), 0);
    assert_eq!(handle.status.cued.load(Ordering::Relaxed), 1);
    assert_eq!(handle.status.underruns.load(Ordering::Relaxed), 0);
    assert!(events(&mut handle).is_empty());
}

#[test]
fn tracks_without_audio_never_go_on_air() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(&mut handle, track(1, &[]), FrameTransition::back_to_back());
    render(&mut mixer, 480, BLOCK_FRAMES);
    assert_eq!(events(&mut handle), [Event::Uncued { entry_id: 1 }]);
    assert!(handle.trash.pop().is_ok());
}

fn cue_over(handle: &mut MixerHandle, track: Box<LoadedTrack>, jingle: Box<LoadedTrack>, at: u64) {
    assert!(
        handle
            .commands
            .push(Command::CueOver {
                track,
                transition: FrameTransition::back_to_back(),
                overlay: Overlay { track: jingle, at },
            })
            .is_ok()
    );
}

fn set_eq(handle: &mut MixerHandle, eq: EqTarget, filter_gains_db: [f32; EQ_BANDS], glide: bool) {
    assert!(
        handle
            .commands
            .push(Command::SetEq {
                eq,
                enabled: true,
                filter_gains_db,
                glide,
            })
            .is_ok()
    );
}

#[test]
fn a_jingle_plays_over_the_intro_and_ends_on_it() {
    let song = tone(20_011, 440.0, 0.4);
    let jingle = tone(3_001, 880.0, 0.3);
    let intro = 7_013_u64;
    let at = intro - 3_001;
    for block in [480, 333, 1] {
        let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
        cue_over(&mut handle, track(1, &song), track(2, &jingle), at);
        let output = render(&mut mixer, 22_000, block);

        let mut expected = song.clone();
        expected.resize(output.len(), 0.0);
        for (index, sample) in jingle.iter().enumerate() {
            expected[at as usize * CHANNELS + index] += sample;
        }
        assert_matches(&output, &expected);

        let events = events(&mut handle);
        assert!(events.contains(&Event::Started {
            deck: Deck::Fx,
            entry_id: 2,
            at_frame: at,
        }));
        assert!(events.contains(&Event::Ended {
            deck: Deck::Fx,
            entry_id: 2,
            played_frames: 3_001,
            at_frame: intro,
            reason: EndReason::Finished,
        }));
    }
}

#[test]
fn skipping_the_track_drops_or_fades_its_jingle() {
    // Skipped before the jingle starts: the jingle never plays.
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue_over(
        &mut handle,
        track(1, &tone(48_000, 440.0, 0.4)),
        track(2, &tone(4_800, 880.0, 0.3)),
        24_000,
    );
    render(&mut mixer, 4_800, BLOCK_FRAMES);
    assert_eq!(mixer.status.overlay.load(Ordering::Relaxed), 2);
    assert!(
        handle
            .commands
            .push(Command::Skip {
                fade_frames: 480,
                curve: FadeCurve::Linear,
            })
            .is_ok()
    );
    render(&mut mixer, 48_000, BLOCK_FRAMES);
    let dropped = events(&mut handle);
    assert!(dropped.contains(&Event::Uncued { entry_id: 2 }));
    assert!(
        !dropped
            .iter()
            .any(|event| matches!(event, Event::Started { entry_id: 2, .. }))
    );
    assert_eq!(mixer.status.overlay.load(Ordering::Relaxed), 0);

    // Skipped while it plays: it fades out with the track.
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue_over(
        &mut handle,
        track(1, &tone(48_000, 440.0, 0.4)),
        track(2, &tone(24_000, 880.0, 0.3)),
        4_800,
    );
    render(&mut mixer, 9_600, BLOCK_FRAMES);
    assert!(
        handle
            .commands
            .push(Command::Skip {
                fade_frames: 480,
                curve: FadeCurve::Linear,
            })
            .is_ok()
    );
    render(&mut mixer, 4_800, BLOCK_FRAMES);
    let events = events(&mut handle);
    assert!(events.contains(&Event::Ended {
        deck: Deck::Fx,
        entry_id: 2,
        played_frames: 4_800 + 480,
        at_frame: 9_600 + 480,
        reason: EndReason::Skipped,
    }));
}

#[test]
fn uncueing_drops_the_jingle_too() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        track(1, &tone(48_000, 440.0, 0.4)),
        FrameTransition::back_to_back(),
    );
    render(&mut mixer, 480, BLOCK_FRAMES);
    cue_over(
        &mut handle,
        track(2, &tone(48_000, 440.0, 0.4)),
        track(3, &tone(4_800, 880.0, 0.3)),
        9_600,
    );
    assert!(handle.commands.push(Command::Uncue).is_ok());
    render(&mut mixer, 480, BLOCK_FRAMES);
    let events = events(&mut handle);
    assert!(events.contains(&Event::Uncued { entry_id: 2 }));
    assert!(events.contains(&Event::Uncued { entry_id: 3 }));
}

/// `samples` through a deck EQ starting at rest.
fn equalized(samples: &[f32], gains_db: [f32; EQ_BANDS]) -> Vec<f32> {
    let mut eq = GraphicEq::new(SAMPLE_RATE);
    eq.restart(true, gains_db);
    let mut output = samples.to_vec();
    eq.process(&mut output);
    output
}

#[test]
fn the_music_eq_filters_both_decks_through_a_crossfade() {
    // The EQ is linear and each deck's starts at rest with its track: the
    // master bus is the crossfade without EQ, through one EQ.
    let gains = [6.0, -3.0, 0.0, 4.0, 0.0, -6.0, 3.0, 0.0, 0.0, 2.0];
    let outgoing = tone(72_000, 440.0, 0.2);
    let incoming = tone(48_000, 3_000.0, 0.2);
    let transition = FrameTransition {
        next_start: 48_000,
        fade_out: Some(FrameRamp {
            start: 48_000,
            length: 12_000,
            curve: FadeCurve::Linear,
            level: 0.0,
        }),
    };
    let crossfade = |eq: bool| {
        let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
        if eq {
            set_eq(&mut handle, EqTarget::Music, gains, false);
        }
        cue(
            &mut handle,
            track(1, &outgoing),
            FrameTransition::back_to_back(),
        );
        let mut output = render(&mut mixer, 480, BLOCK_FRAMES);
        cue(&mut handle, track(2, &incoming), transition);
        output.extend(render(&mut mixer, 96_000 - 480, BLOCK_FRAMES));
        output
    };
    let plain = crossfade(false);
    assert_matches(&crossfade(true), &equalized(&plain, gains));
    // And it does change the sound.
    assert!(
        plain
            .iter()
            .zip(&crossfade(true))
            .any(|(a, b)| (a - b).abs() > 0.01)
    );
}

#[test]
fn the_fx_eq_leaves_the_decks_alone() {
    let gains = [12.0; EQ_BANDS];
    let music = tone(9_600, 440.0, 0.1);
    let jingle = tone(4_800, 1_000.0, 0.05);
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    set_eq(&mut handle, EqTarget::Fx, gains, false);
    cue(
        &mut handle,
        track(1, &music),
        FrameTransition::back_to_back(),
    );
    assert!(
        handle
            .commands
            .push(Command::PlayFx(track(2, &jingle)))
            .is_ok()
    );
    let output = render(&mut mixer, 9_600, BLOCK_FRAMES);
    let filtered = equalized(&jingle, gains);
    let expected: Vec<f32> = music
        .iter()
        .enumerate()
        .map(|(index, sample)| sample + filtered.get(index).copied().unwrap_or(0.0))
        .collect();
    assert_matches(&output, &expected);
}

#[test]
fn eq_changes_glide_in() {
    // The 1 kHz band to +12 dB on a 1 kHz tone, while it plays.
    let mut gains = [0.0; EQ_BANDS];
    gains[4] = 12.0;
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    cue(
        &mut handle,
        track(1, &tone(48_000, 1_000.0, 0.1)),
        FrameTransition::back_to_back(),
    );
    render(&mut mixer, 9_600, BLOCK_FRAMES);
    set_eq(&mut handle, EqTarget::Music, gains, true);
    let output = render(&mut mixer, 14_400, BLOCK_FRAMES);
    let peak = |samples: &[f32]| samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    // Not at once: the first millisecond is still close to the level
    // before, and the level rises millisecond after millisecond.
    let peaks: Vec<f32> = output.chunks(48 * CHANNELS).take(50).map(peak).collect();
    assert!(peaks[0] < 0.12, "{}", peaks[0]);
    assert!(peaks.windows(2).all(|pair| pair[1] >= pair[0] - 1e-4));
    // There after 200 ms: 0.1 × 10^(12/20).
    let level = peak(&output[9_600 * CHANNELS..]);
    assert!((level - 0.398).abs() < 0.005, "{level}");
}

fn set_agc(handle: &mut MixerHandle, params: ChainParams, glide: bool) {
    assert!(
        handle
            .commands
            .push(Command::SetAgc { params, glide })
            .is_ok()
    );
}

/// The chain on, with its defaults: the final limiter alone.
fn chain_on() -> ChainParams {
    ChainParams {
        bypass_all: false,
        ..ChainParams::default()
    }
}

#[test]
fn the_agc_chain_limits_the_master_bus() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    set_agc(&mut handle, chain_on(), false);
    let mut loud = track(1, &tone(48_000, 440.0, 0.9));
    // 12 dB over full scale.
    loud.gain = 4.0;
    cue(&mut handle, loud, FrameTransition::back_to_back());
    let output = render(&mut mixer, 48_000, BLOCK_FRAMES);
    let ceiling = 10f32.powf(-1.0 / 20.0);
    assert!(output.iter().all(|sample| sample.abs() <= ceiling));
    // Limited, not clipped: the peaks come down smoothly, the sine keeps its
    // shape (few samples at the ceiling).
    let at_ceiling = output.iter().filter(|s| s.abs() > ceiling * 0.999).count();
    assert!(at_ceiling < output.len() / 20, "{at_ceiling}");
    let agc = &mixer.status.agc;
    assert!(agc.active.load(Ordering::Relaxed));
    assert_eq!(agc.latency_frames.load(Ordering::Relaxed), 239);
    let (_, _, final_stage) = agc.reductions_db();
    assert!(final_stage < -10.0, "{final_stage}");
    // Its input is the master bus before it, over full scale.
    let (peak, _) = agc.input.read();
    assert!(peak[0] > 3.0, "{peak:?}");
}

#[test]
fn a_bypassed_chain_reports_nothing_and_delays_nothing() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    let samples = tone(9_600, 440.0, 0.5);
    cue(
        &mut handle,
        track(1, &samples),
        FrameTransition::back_to_back(),
    );
    let output = render(&mut mixer, 9_600, BLOCK_FRAMES);
    assert_eq!(output, samples);
    let agc = &mixer.status.agc;
    assert!(!agc.active.load(Ordering::Relaxed));
    assert_eq!(agc.latency_frames.load(Ordering::Relaxed), 0);
    assert_eq!(agc.agc_gain_db(), None);
}

#[test]
fn silence_is_measured_before_the_agc_chain() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    let mut params = chain_on();
    params.gated_agc.enabled = true;
    params.gated_agc.gate_lufs = -70.0;
    params.gated_agc.max_gain_db = 18.0;
    set_agc(&mut handle, params, false);
    // −54 dBFS: silence for the watchdog (under −50 dBFS).
    let frames = 48_000 * 8;
    cue(
        &mut handle,
        track(1, &tone(frames, 440.0, 0.002)),
        FrameTransition::back_to_back(),
    );
    render(&mut mixer, frames, BLOCK_FRAMES);
    let status = &mixer.status;
    // The AGC lifted it over the silence level on the master bus...
    let gain = status.agc.agc_gain_db().unwrap_or_default();
    assert!(gain > 12.0, "{gain}");
    assert!(status.master.read().0[0] > SILENCE_LEVEL);
    // ...yet the watchdog sees the silence the decks play.
    assert_eq!(status.silent_frames.load(Ordering::Relaxed), frames as u64);
}

#[test]
fn the_agc_starts_from_the_gain_handed_over() {
    let (mut mixer, mut handle) = Mixer::new(BLOCK_FRAMES);
    let mut params = chain_on();
    params.gated_agc.enabled = true;
    params.final_stage.enabled = false;
    set_agc(&mut handle, params, false);
    assert!(
        handle
            .commands
            .push(Command::StartAgcAt { gain_db: 4.0 })
            .is_ok()
    );
    let samples = tone(4_800, 440.0, 0.1);
    cue(
        &mut handle,
        track(1, &samples),
        FrameTransition::back_to_back(),
    );
    let output = render(&mut mixer, 4_800, BLOCK_FRAMES);
    let gain = 10f32.powf(4.0 / 20.0);
    for (out, sample) in output.iter().zip(&samples) {
        assert!((out - sample * gain).abs() < 1e-5);
    }
    assert_eq!(mixer.status.agc.agc_gain_db(), Some(4.0));
}
