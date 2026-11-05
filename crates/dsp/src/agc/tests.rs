// SPDX-License-Identifier: AGPL-3.0-or-later

use std::f64::consts::TAU;

use super::crossover::Crossover;
use super::dynamics::Dynamics;
use super::gate::LoudnessMeter;
use super::*;
use crate::eq::{Coefficients, SHELF_Q};

const RATE: u32 = 48_000;
const RATE_F: f64 = 48_000.0;
const SECOND: usize = 48_000;

// --- Signals and measures

/// Stereo, `left` and `right` the amplitudes of each channel's sine.
fn sine_lr(frequency: f64, left: f64, right: f64, seconds: f64) -> Vec<f32> {
    let frames = (seconds * RATE_F) as usize;
    let mut out = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let x = (TAU * frequency * n as f64 / RATE_F).sin();
        out.push((left * x) as f32);
        out.push((right * x) as f32);
    }
    out
}

fn sine(frequency: f64, amplitude: f64, seconds: f64) -> Vec<f32> {
    sine_lr(frequency, amplitude, amplitude, seconds)
}

fn noise(frames: usize, amplitude: f32) -> Vec<f32> {
    let mut seed = 12_345u32;
    (0..frames * 2)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amplitude
        })
        .collect()
}

fn run(block: &mut impl Block, samples: &mut [f32]) {
    for chunk in samples.chunks_mut(480 * 2) {
        block.process(chunk);
    }
}

fn run_chain(chain: &mut Chain, samples: &mut [f32]) {
    for chunk in samples.chunks_mut(480 * 2) {
        chain.process(chunk);
    }
}

fn channel(samples: &[f32], channel: usize) -> Vec<f32> {
    samples.iter().skip(channel).step_by(2).copied().collect()
}

fn rms_db(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| f64::from(s).powi(2)).sum();
    10.0 * (sum / samples.len() as f64).max(1e-30).log10()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |peak, s| peak.max(s.abs()))
}

/// The last second of interleaved stereo.
fn last_second(samples: &[f32]) -> &[f32] {
    &samples[samples.len() - SECOND * 2..]
}

/// Loudness (LUFS) of interleaved stereo, over its last 3 s.
fn loudness(samples: &[f32]) -> f32 {
    let mut meter = LoudnessMeter::new(RATE_F);
    for frame in samples.as_chunks::<2>().0 {
        meter.add(*frame);
    }
    meter.loudness(30)
}

/// Side signal (L − R) / 2 of interleaved stereo.
fn side(samples: &[f32]) -> Vec<f32> {
    samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| (f[0] - f[1]) * 0.5)
        .collect()
}

fn mid(samples: &[f32]) -> Vec<f32> {
    samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| (f[0] + f[1]) * 0.5)
        .collect()
}

/// A band that passes everything.
fn neutral_band() -> BandParams {
    BandParams {
        threshold_db: 0.0,
        ratio: 1.0,
        ..BandParams::default()
    }
}

// --- Gated AGC

#[test]
fn the_loudness_meter_reads_a_sine_on_both_channels_at_its_peak_level() {
    // EBU Tech 3341, test 1: a 1 kHz sine at −23 dBFS on both channels reads
    // −23 LUFS.
    for level in [-23.0, 0.0, -6.0] {
        let reading = loudness(&sine(1_000.0, 10f64.powf(level / 20.0), 3.0));
        assert!(
            (f64::from(reading) - level).abs() < 0.05,
            "{level}: {reading}"
        );
    }
}

/// A 1 kHz stereo sine of this loudness.
fn sine_at_lufs(lufs: f64, seconds: f64) -> Vec<f32> {
    sine(1_000.0, 10f64.powf(lufs / 20.0), seconds)
}

fn agc(gate_lufs: f32, max_gain_db: f32) -> GatedAgc {
    let mut agc = GatedAgc::new(RATE);
    agc.set(&GatedAgcParams {
        enabled: true,
        target_lufs: -16.0,
        gate_lufs,
        max_gain_db,
    });
    agc
}

#[test]
fn the_agc_brings_the_programme_to_the_target() {
    for (lufs, gain) in [(-22.0, 6.0), (-10.0, -6.0), (-16.0, 0.0)] {
        let mut agc = agc(-45.0, 12.0);
        let mut audio = sine_at_lufs(lufs, 40.0);
        run(&mut agc, &mut audio);
        let out = loudness(&audio);
        assert!((out + 16.0).abs() < 0.5, "{lufs} LUFS in, {out} out");
        assert!(
            (agc.gain_db() - gain as f32).abs() < 0.5,
            "{}",
            agc.gain_db()
        );
    }
}

#[test]
fn the_agc_never_goes_beyond_its_maximum_gain() {
    for (lufs, max) in [(-35.0, 6.0), (0.0, 6.0), (-35.0, 0.0)] {
        let mut agc = agc(-60.0, max);
        let mut audio = sine_at_lufs(lufs, 40.0);
        let mut furthest = 0.0f32;
        for chunk in audio.chunks_mut(480 * 2) {
            agc.process(chunk);
            furthest = furthest.max(agc.gain_db().abs());
        }
        assert!(furthest <= max + 1e-4, "{furthest}");
        assert!((agc.gain_db().abs() - max).abs() < 0.3, "{}", agc.gain_db());
    }
}

#[test]
fn the_agc_holds_its_gain_under_the_gate() {
    let mut agc = agc(-45.0, 12.0);
    let mut audio = sine_at_lufs(-50.0, 20.0);
    run(&mut agc, &mut audio);
    assert_eq!(agc.gain_db(), 0.0);

    // Music, then a pause: the gain stays where the music left it.
    let mut agc = self::agc(-45.0, 12.0);
    let mut music = sine_at_lufs(-22.0, 10.0);
    run(&mut agc, &mut music);
    let before = agc.gain_db();
    assert!(before > 2.0, "{before}");
    let mut silence = vec![0.0; SECOND * 2 * 10];
    run(&mut agc, &mut silence);
    assert!(
        (agc.gain_db() - before).abs() < 0.3,
        "{before} {}",
        agc.gain_db()
    );
}

#[test]
fn the_agc_starts_from_the_gain_it_is_given() {
    let mut agc = agc(-45.0, 6.0);
    agc.start_at(4.0);
    let mut audio = sine_at_lufs(-30.0, 0.05);
    let input = audio.clone();
    run(&mut agc, &mut audio);
    let gain = rms_db(&audio) - rms_db(&input);
    assert!((gain - 4.0).abs() < 0.01, "{gain}");
    agc.start_at(9.0);
    assert_eq!(agc.gain_db(), 6.0);
}

// --- Stereo expander

fn expander(level: f32, depth: f32, threshold_db: f32) -> StereoExpander {
    let mut expander = StereoExpander::new(RATE);
    expander.set(&StereoExpanderParams {
        enabled: true,
        level,
        depth,
        threshold_db,
    });
    expander
}

/// Side boost (dB) of a 1 kHz sine whose side is `width_db` under its mid.
fn side_boost(mut expander: StereoExpander, frequency: f64, amplitude: f64, width_db: f64) -> f64 {
    let s = 10f64.powf(width_db / 20.0);
    let input = sine_lr(frequency, amplitude * (1.0 + s), amplitude * (1.0 - s), 2.0);
    let mut audio = input.clone();
    run(&mut expander, &mut audio);
    let mids = rms_db(&mid(last_second(&audio))) - rms_db(&mid(last_second(&input)));
    assert!(mids.abs() < 0.01, "the mid moved by {mids} dB");
    rms_db(&side(last_second(&audio))) - rms_db(&side(last_second(&input)))
}

#[test]
fn a_mono_signal_stays_mono() {
    let mut expander = expander(100.0, 100.0, -60.0);
    let input = sine(1_000.0, 0.5, 1.0);
    let mut audio = input.clone();
    run(&mut expander, &mut audio);
    assert_eq!(audio, input);
}

#[test]
fn a_near_mono_signal_gets_its_side_boost() {
    // 20 dB narrow: depth 50 % asks for 7 dB; level 50 % allows 3.
    let boost = side_boost(expander(50.0, 50.0, -60.0), 1_000.0, 0.3, -20.0);
    assert!((boost - 3.0).abs() < 0.3, "{boost}");
    let boost = side_boost(expander(100.0, 100.0, -60.0), 1_000.0, 0.3, -20.0);
    assert!((boost - 6.0).abs() < 0.3, "{boost}");
    // 12 dB narrow, depth 25 %: 1.5 dB.
    let boost = side_boost(expander(100.0, 25.0, -60.0), 1_000.0, 0.3, -12.0);
    assert!((boost - 1.5).abs() < 0.3, "{boost}");
}

#[test]
fn wide_quiet_and_bass_signals_are_left_alone() {
    let wide = side_boost(expander(100.0, 100.0, -60.0), 1_000.0, 0.3, -4.0);
    assert!(wide.abs() < 0.01, "{wide}");
    let quiet = side_boost(expander(100.0, 100.0, -40.0), 1_000.0, 0.003, -20.0);
    assert!(quiet.abs() < 0.01, "{quiet}");
    let bass = side_boost(expander(100.0, 100.0, -60.0), 40.0, 0.3, -20.0);
    assert!(bass < 0.5, "{bass}");
}

// --- Bass EQ

fn bass_response_db(params: BassEqParams, frequency: f64) -> f64 {
    let mut eq = BassEq::new(RATE);
    eq.set(&params);
    eq.reset();
    let input = sine(frequency, 0.1, 2.0);
    let mut audio = input.clone();
    run(&mut eq, &mut audio);
    rms_db(last_second(&audio)) - rms_db(last_second(&input))
}

#[test]
fn the_bass_eq_matches_the_filter_design() {
    for (shape, frequency, gain_db) in [
        (BassShape::Shelf, 100.0f32, 6.0f32),
        (BassShape::Shelf, 40.0, -12.0),
        (BassShape::Peak, 80.0, 12.0),
        (BassShape::Peak, 250.0, -6.0),
    ] {
        let params = BassEqParams {
            enabled: true,
            shape,
            frequency_hz: frequency,
            gain_db,
        };
        let design = match shape {
            BassShape::Shelf => Coefficients::shelf(
                RATE_F,
                f64::from(frequency),
                SHELF_Q,
                f64::from(gain_db),
                false,
            ),
            BassShape::Peak => {
                Coefficients::peaking(RATE_F, f64::from(frequency), 1.0, f64::from(gain_db))
            }
        };
        for probe in [20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 5_000.0, 20_000.0] {
            let measured = bass_response_db(params, probe);
            let expected = design.response_db(RATE_F, probe);
            assert!(
                (measured - expected).abs() < 0.1,
                "{shape:?} {frequency} Hz {gain_db} dB at {probe} Hz: {measured} for {expected}"
            );
        }
    }
}

// --- Crossovers

#[test]
fn each_band_of_a_crossover_is_6_db_down_at_its_frequency() {
    let mut crossover = Crossover::new(1_000.0, RATE_F);
    let input: Vec<f32> = channel(&sine(1_000.0, 0.5, 2.0), 0);
    let (mut low, mut high) = (Vec::new(), Vec::new());
    for &x in &input {
        let (l, h) = crossover.split(0, f64::from(x));
        low.push(l as f32);
        high.push(h as f32);
    }
    let tail = input.len() - SECOND;
    for band in [&low, &high] {
        let level = rms_db(&band[tail..]) - rms_db(&input[tail..]);
        assert!((level + 6.02).abs() < 0.1, "{level}");
    }
}

fn flat_five() -> FiveBandParams {
    FiveBandParams {
        enabled: true,
        bands: [neutral_band(); 5],
        ..FiveBandParams::default()
    }
}

fn flat_dual() -> DualBandParams {
    DualBandParams {
        enabled: true,
        bands: [neutral_band(); 2],
        ..DualBandParams::default()
    }
}

#[test]
fn flat_bands_add_up_to_the_input() {
    for frequency in [
        20.0, 50.0, 100.0, 200.0, 400.0, 1_000.0, 1_600.0, 3_000.0, 6_400.0, 10_000.0, 20_000.0,
    ] {
        let input = sine(frequency, 0.25, 2.0);
        let mut five = Multiband::new(RATE, 5);
        five.set_five(&flat_five());
        five.reset();
        let mut audio = input.clone();
        run(&mut five, &mut audio);
        let error = rms_db(last_second(&audio)) - rms_db(last_second(&input));
        assert!(error.abs() < 0.1, "5 bands at {frequency} Hz: {error} dB");

        let mut dual = Multiband::new(RATE, 2);
        dual.set_dual(&flat_dual());
        dual.reset();
        let mut audio = input.clone();
        run(&mut dual, &mut audio);
        let error = rms_db(last_second(&audio)) - rms_db(last_second(&input));
        assert!(error.abs() < 0.1, "2 bands at {frequency} Hz: {error} dB");
    }
}

// --- Band dynamics

fn dynamics(params: BandParams) -> Dynamics {
    let mut dynamics = Dynamics::new(RATE_F);
    dynamics.set(&params);
    dynamics.settle();
    dynamics
}

/// The gain at a steady level, once settled.
fn settled_gain(params: BandParams, level_db: f32) -> f32 {
    let mut dynamics = dynamics(params);
    let level = db_to_gain(level_db);
    let mut gain = 0.0;
    for _ in 0..SECOND * 3 {
        gain = dynamics.step(level);
    }
    gain
}

#[test]
fn the_static_curves_follow_threshold_and_ratio() {
    let fast = BandParams {
        attack_ms: 1.0,
        release_ms: 10.0,
        ..BandParams::default()
    };
    let compressor = BandParams {
        threshold_db: -20.0,
        ratio: 4.0,
        ..fast
    };
    // 12 dB over: 3 dB come out, a gain of −9 dB.
    assert!((settled_gain(compressor, -8.0) + 9.0).abs() < 0.5);
    assert!(settled_gain(compressor, -30.0).abs() < 0.01);

    let expander = BandParams {
        mode: BandMode::Expander,
        threshold_db: -40.0,
        ratio: 2.0,
        ..fast
    };
    // 10 dB under: 20 dB under once expanded, a gain of −10 dB.
    assert!((settled_gain(expander, -50.0) + 10.0).abs() < 0.5);
    assert!(settled_gain(expander, -20.0).abs() < 0.01);
    // At most 40 dB down.
    assert!((settled_gain(expander, -120.0) + 40.0).abs() < 0.01);

    let limiter = BandParams {
        mode: BandMode::Limiter,
        threshold_db: -10.0,
        ratio: 2.0,
        ..fast
    };
    // The ratio does not count: down to the threshold.
    assert!((settled_gain(limiter, -4.0) + 6.0).abs() < 0.5);
    assert!(settled_gain(limiter, -11.0).abs() < 0.01);
}

/// Frames until the gain is 63 % of the way from `from` to `to`.
fn frames_to_63_percent(dynamics: &mut Dynamics, level: f32, from: f32, to: f32) -> usize {
    let mark = from + (to - from) * 0.632;
    let mut frames = 0;
    loop {
        let gain = dynamics.step(level);
        frames += 1;
        if (to < from && gain <= mark) || (to > from && gain >= mark) || frames > SECOND * 10 {
            return frames;
        }
    }
}

#[test]
fn attack_hold_and_release_take_their_time() {
    let params = BandParams {
        mode: BandMode::Limiter,
        threshold_db: -20.0,
        attack_ms: 10.0,
        release_ms: 100.0,
        hold_ms: 20.0,
        ..BandParams::default()
    };
    let mut band = dynamics(params);
    let loud = db_to_gain(-10.0);
    let quiet = db_to_gain(-40.0);
    let attack = frames_to_63_percent(&mut band, loud, 0.0, -10.0);
    assert!((432..=528).contains(&attack), "attack: {attack} frames");
    for _ in 0..SECOND {
        band.step(loud);
    }
    let held = band.step(quiet);
    for frame in 0..958 {
        assert_eq!(band.step(quiet), held, "frame {frame} of the hold");
    }
    band.step(quiet);
    let release = frames_to_63_percent(&mut band, quiet, held, 0.0);
    assert!(
        (4_320..=5_280).contains(&release),
        "release: {release} frames"
    );
}

#[test]
fn linked_bands_share_the_deeper_gain_reduction() {
    let mut gains = [-6.0, -2.0, -1.0, 0.0, -3.0];
    Multiband::link(5, &[true, false, true, true], &mut gains);
    assert_eq!(gains, [-6.0, -6.0, -3.0, -3.0, -3.0]);

    // In the processor: a loud bass makes the linked band above come down.
    let band = BandParams {
        threshold_db: -30.0,
        ratio: 4.0,
        ..BandParams::default()
    };
    let gain_at_1k = |link: bool| {
        let mut dual = Multiband::new(RATE, 2);
        dual.set_dual(&DualBandParams {
            enabled: true,
            crossover_hz: 200.0,
            link,
            bands: [band, band],
        });
        dual.reset();
        let input = sine(50.0, 0.8, 2.0)
            .iter()
            .zip(sine(1_000.0, 0.01, 2.0))
            .map(|(a, b)| a + b)
            .collect::<Vec<_>>();
        let mut audio = input.clone();
        run(&mut dual, &mut audio);
        dual.take_deepest_db()[1]
    };
    assert!(gain_at_1k(false) > -0.5);
    assert!(gain_at_1k(true) < -10.0);
}

#[test]
fn links_changed_again_mid_glide_go_on_from_where_they_are() {
    let mut dual = Multiband::new(RATE, 2);
    let set = |dual: &mut Multiband, link: bool| {
        dual.set_dual(&DualBandParams {
            enabled: true,
            link,
            ..DualBandParams::default()
        });
    };
    // Each band on its own: −20 and 0 dB.
    let independent = [-20.0, 0.0, 0.0, 0.0, 0.0];
    set(&mut dual, false);
    assert_eq!(dual.linked(&independent)[1], 0.0);
    // Linked: the upper band glides down to −20 dB over 50 ms…
    set(&mut dual, true);
    let mut upper = 0.0;
    for _ in 0..(SECOND / 20 / 4) {
        let next = dual.linked(&independent)[1];
        assert!((next - upper).abs() < 0.1, "{upper} to {next} dB");
        upper = next;
    }
    assert!(
        (-6.0..=-4.0).contains(&upper),
        "a quarter of the way: {upper} dB"
    );
    // …unlinked a quarter of the way: back up from there, without a jump.
    set(&mut dual, false);
    for _ in 0..SECOND / 10 {
        let next = dual.linked(&independent)[1];
        assert!((next - upper).abs() < 0.1, "{upper} to {next} dB");
        upper = next;
    }
    assert_eq!(upper, 0.0);
}

/// Harmonic distortion (%) of a sine through one band compressed by about
/// 10 dB with the default times.
fn compressed_sine_distortion(frequency: f64) -> f64 {
    let mut band = dynamics(BandParams::default());
    // −20 dB threshold, 3:1: 15 dB over gives −10 dB.
    let amplitude = 10f64.powf(-5.0 / 20.0);
    let frames = SECOND * 3;
    let out: Vec<f64> = (0..frames)
        .map(|n| {
            let x = (amplitude * (TAU * frequency * n as f64 / RATE_F).sin()) as f32;
            f64::from(x * db_to_gain(band.step(x.abs())))
        })
        .collect();
    let tail = &out[frames - SECOND..];
    let magnitude = |harmonic: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &y) in tail.iter().enumerate() {
            let phase = TAU * frequency * harmonic * n as f64 / RATE_F;
            re += y * phase.cos();
            im += y * phase.sin();
        }
        (re * re + im * im).sqrt()
    };
    let fundamental = magnitude(1.0);
    let harmonics: f64 = (2..=10)
        .map(f64::from)
        .filter(|h| frequency * h < RATE_F / 2.0)
        .map(|h| magnitude(h).powi(2))
        .sum();
    100.0 * harmonics.sqrt() / fundamental
}

#[test]
fn compression_distorts_little() {
    // Measured: 0.35, 0.18 and 0.02 %.
    for (frequency, most) in [(50.0, 0.5), (100.0, 0.25), (1_000.0, 0.05)] {
        let distortion = compressed_sine_distortion(frequency);
        assert!(distortion < most, "{frequency} Hz: {distortion:.3} %");
    }
}

// --- Final stage

fn final_stage(mode: FinalMode) -> FinalStage {
    let mut stage = FinalStage::new(RATE);
    stage.set(&FinalStageParams {
        enabled: true,
        mode,
        ceiling_db: -1.0,
        release_ms: 100.0,
    });
    stage.reset();
    stage
}

fn loud_signals() -> Vec<Vec<f32>> {
    let square = |amplitude: f32| {
        sine(440.0, 1.0, 1.0)
            .iter()
            .map(|s| amplitude * s.signum())
            .collect::<Vec<_>>()
    };
    let mut burst = sine(440.0, 0.3, 2.0);
    for sample in &mut burst[SECOND / 2..SECOND] {
        *sample *= 16.0;
    }
    vec![
        square(1.0),
        square(4.0),
        sine(1_000.0, 1.0, 1.0),
        sine(100.0, 16.0, 1.0),
        sine(10_000.0, 16.0, 1.0),
        burst,
        noise(SECOND, 4.0),
    ]
}

#[test]
fn the_final_stage_never_goes_over_its_ceiling() {
    let ceiling = db_to_gain(-1.0);
    for mode in [FinalMode::Limiter, FinalMode::Clipper] {
        for (index, signal) in loud_signals().into_iter().enumerate() {
            let mut audio = signal;
            run(&mut final_stage(mode), &mut audio);
            assert!(
                peak(&audio) <= ceiling,
                "{mode:?}, signal {index}: {}",
                peak(&audio)
            );
        }
    }
}

#[test]
fn under_the_ceiling_the_limiter_is_a_delay_and_the_clipper_nothing() {
    let input = sine(440.0, 0.8, 1.0);
    let mut limiter = final_stage(FinalMode::Limiter);
    assert_eq!(limiter.latency(), 239);
    let mut audio = input.clone();
    run(&mut limiter, &mut audio);
    assert!(audio[..239 * 2].iter().all(|&s| s == 0.0));
    assert_eq!(&audio[239 * 2..], &input[..input.len() - 239 * 2]);

    let mut clipper = final_stage(FinalMode::Clipper);
    assert_eq!(
        clipper.latency(),
        0,
        "no delay once the clipper alone is heard"
    );
    let mut audio = input.clone();
    run(&mut clipper, &mut audio);
    assert_eq!(audio, input);
    // Crossfading from the limiter: its delay still counts.
    clipper.set(&FinalStageParams {
        enabled: true,
        mode: FinalMode::Limiter,
        ceiling_db: -1.0,
        release_ms: 100.0,
    });
    run(&mut clipper, &mut sine(440.0, 0.8, 0.01));
    assert_eq!(clipper.latency(), 239);
}

#[test]
fn the_limiter_limits_rather_than_clips_and_recovers() {
    let mut audio = sine(440.0, 0.3, 3.0);
    for sample in &mut audio[SECOND / 2..SECOND] {
        *sample *= 8.0;
    }
    let mut limiter = final_stage(FinalMode::Limiter);
    run(&mut limiter, &mut audio);
    // Limited, not clipped: each peak reaches the ceiling, but a clipped sine
    // 18 dB over would sit there most of the time.
    let ceiling = db_to_gain(-1.0);
    let at_ceiling = audio.iter().filter(|s| s.abs() > ceiling * 0.999).count();
    assert!(
        at_ceiling < SECOND / 20,
        "{at_ceiling} samples at the ceiling"
    );
    // The burst peaks at 2.4: the gain goes down to the ceiling over it.
    let deepest = limiter.take_lowest_db();
    let expected = gain_to_db(ceiling / 2.4);
    assert!((deepest - expected).abs() < 0.1, "{deepest} for {expected}");
    // Back to unity long after.
    assert!((f64::from(peak(last_second(&audio))) - 0.3).abs() < 0.001);
}

// --- The chain

fn heavy() -> ChainParams {
    let band = BandParams {
        threshold_db: -24.0,
        ratio: 4.0,
        attack_ms: 5.0,
        release_ms: 150.0,
        hold_ms: 10.0,
        gain_db: 2.0,
        ..BandParams::default()
    };
    ChainParams {
        bypass_all: false,
        gated_agc: GatedAgcParams {
            enabled: true,
            max_gain_db: 12.0,
            ..GatedAgcParams::default()
        },
        stereo_expander: StereoExpanderParams {
            enabled: true,
            level: 100.0,
            depth: 100.0,
            threshold_db: -50.0,
        },
        bass_eq: BassEqParams {
            enabled: true,
            shape: BassShape::Shelf,
            frequency_hz: 100.0,
            gain_db: 4.0,
        },
        five_band: FiveBandParams {
            enabled: true,
            overdrive_db: 6.0,
            links: [true, false, false, true],
            bands: [band; 5],
            ..FiveBandParams::default()
        },
        dual_band: DualBandParams {
            enabled: true,
            bands: [band; 2],
            ..DualBandParams::default()
        },
        final_stage: FinalStageParams::default(),
    }
}

/// Every block off.
fn all_off() -> ChainParams {
    let mut params = heavy();
    for switch in SWITCHES {
        switch(&mut params);
    }
    params
}

type Switch = fn(&mut ChainParams);

const SWITCHES: [Switch; 6] = [
    |p| p.gated_agc.enabled = false,
    |p| p.stereo_expander.enabled = false,
    |p| p.bass_eq.enabled = false,
    |p| p.five_band.enabled = false,
    |p| p.dual_band.enabled = false,
    |p| p.final_stage.enabled = false,
];

/// Two seconds of a near-mono music-like signal: a few sines and noise.
fn programme(seconds: f64) -> Vec<f32> {
    let mut audio = sine_lr(110.0, 0.3, 0.25, seconds);
    for (frequency, amplitude) in [(440.0, 0.15), (1_500.0, 0.1), (5_000.0, 0.05)] {
        for (sample, add) in audio.iter_mut().zip(sine(frequency, amplitude, seconds)) {
            *sample += add;
        }
    }
    for (sample, add) in audio
        .iter_mut()
        .zip(noise((seconds * RATE_F) as usize, 0.02))
    {
        *sample += add;
    }
    audio
}

#[test]
fn a_bypassed_chain_leaves_the_audio_as_it_is() {
    let input = programme(1.0);
    let mut chain = Chain::new(RATE);
    assert!(!chain.is_active());
    let mut audio = input.clone();
    run_chain(&mut chain, &mut audio);
    assert_eq!(audio, input, "bypassed");

    // The final stage, on by default, fades out first.
    chain.set(&all_off());
    run_chain(&mut chain, &mut programme(0.1));
    let mut audio = input.clone();
    run_chain(&mut chain, &mut audio);
    assert_eq!(audio, input, "on, every block off");

    chain.set(&heavy());
    run_chain(&mut chain, &mut programme(2.0));
    chain.set(&ChainParams {
        bypass_all: true,
        ..heavy()
    });
    run_chain(&mut chain, &mut programme(1.0));
    assert!(!chain.is_active());
    let mut audio = input.clone();
    run_chain(&mut chain, &mut audio);
    assert_eq!(audio, input, "bypassed after playing");

    // Bypassed within one call (longer than the 50 ms fade): what comes
    // after the fade goes out untouched, not through the chain again.
    chain.set(&heavy());
    run_chain(&mut chain, &mut programme(2.0));
    chain.set(&ChainParams {
        bypass_all: true,
        ..heavy()
    });
    let input = programme(0.2);
    let mut audio = input.clone();
    chain.process(&mut audio);
    assert!(!chain.is_active());
    let after_the_fade = SECOND / 10 * 2;
    assert_eq!(&audio[after_the_fade..], &input[after_the_fade..]);
}

#[test]
fn each_block_with_neutral_settings_changes_little() {
    for keep in 0..SWITCHES.len() {
        let mut params = heavy();
        for (index, switch) in SWITCHES.iter().enumerate() {
            if index != keep {
                switch(&mut params);
            }
        }
        params.gated_agc.max_gain_db = 0.0;
        params.stereo_expander.level = 0.0;
        params.bass_eq.gain_db = 0.0;
        params.five_band.overdrive_db = 0.0;
        params.five_band.bands = [neutral_band(); 5];
        params.dual_band.bands = [neutral_band(); 2];
        let mut chain = Chain::new(RATE);
        chain.restart(&params);
        let input = sine_lr(1_000.0, 0.03, 0.02, 2.0);
        let mut audio = input.clone();
        run_chain(&mut chain, &mut audio);
        let change = rms_db(last_second(&audio)) - rms_db(last_second(&input));
        assert!(change.abs() < 0.1, "block {keep}: {change} dB");
    }
}

#[test]
fn a_loud_heavy_chain_stays_under_the_ceiling() {
    let mut chain = Chain::new(RATE);
    chain.restart(&heavy());
    assert_eq!(chain.latency(), 239);
    for signal in loud_signals() {
        let mut audio = signal;
        run_chain(&mut chain, &mut audio);
        assert!(peak(&audio) <= db_to_gain(-1.0));
    }
}

/// RMS (dB) of the left channel above 3 kHz: a click spreads there, a 300 Hz
/// sine and its first harmonics hardly do. The first quarter of the window,
/// where the filter settles, is left out.
fn high_db(samples: &[f32]) -> f64 {
    let mut crossover = Crossover::new(3_000.0, RATE_F);
    let high: Vec<f32> = channel(samples, 0)
        .iter()
        .map(|&x| crossover.split(0, f64::from(x)).1 as f32)
        .collect();
    rms_db(&high[high.len() / 4..])
}

/// What a change does to a sine around it.
#[derive(Debug)]
struct Change {
    /// Energy above 3 kHz around the change, against the signal's (dB).
    high_dbc: f64,
    /// How much more energy above 3 kHz there is around the change than in
    /// the steady sound before and after it (dB).
    excess_db: f64,
}

impl Change {
    /// Clicks are energy above 3 kHz, more than in the steady sound and loud
    /// enough to be heard.
    fn clicks(&self) -> bool {
        self.excess_db > 10.0 && self.high_dbc > -70.0
    }
}

/// A sine through a chain set to `before`, `change` applied after 2 s.
fn change(before: &ChainParams, amplitude: f64, change: impl FnOnce(&mut Chain)) -> Change {
    let mut chain = Chain::new(RATE);
    chain.restart(before);
    let mut audio = sine_lr(300.0, amplitude, amplitude * 0.75, 4.0);
    let (first, second) = audio.split_at_mut(SECOND * 2 * 2);
    run_chain(&mut chain, first);
    change(&mut chain);
    run_chain(&mut chain, second);
    let window = SECOND / 5 * 2;
    // The change and the 200 ms before it, so the filter is settled there.
    let mut around = first[first.len() - window..].to_vec();
    around.extend_from_slice(&second[..window]);
    let before_part = &first[first.len() - window * 2..first.len() - window];
    let after_part = &second[second.len() - window..];
    let steady = high_db(before_part).max(high_db(after_part));
    let around_part = &around[window / 2..];
    Change {
        high_dbc: high_db(around_part) - rms_db(&channel(around_part, 0)),
        excess_db: high_db(around_part) - steady,
    }
}

fn assert_glides(name: &str, change: Change) {
    eprintln!("CLICK {name}: {change:?}");
    if std::env::var("PROBE").is_err() {
        assert!(!change.clicks(), "{name} clicks: {change:?}");
    }
}

fn assert_clicks(name: &str, change: Change) {
    eprintln!("CLICK control {name}: {change:?}");
    if std::env::var("PROBE").is_err() {
        assert!(
            change.clicks(),
            "{name}, made at once, does not click: {change:?}"
        );
    }
}

#[test]
fn switching_does_not_click() {
    let set = |params: ChainParams| move |chain: &mut Chain| chain.set(&params);
    let at_once = |params: ChainParams| move |chain: &mut Chain| chain.set_at_once(&params);
    let off = ChainParams::default();
    assert_glides("chain on", change(&off, 0.4, set(heavy())));
    assert_clicks("chain on", change(&off, 0.4, at_once(heavy())));
    let bypassed = ChainParams {
        bypass_all: true,
        ..heavy()
    };
    assert_glides("chain off", change(&heavy(), 0.4, set(bypassed)));
    for (index, switch) in SWITCHES.iter().enumerate() {
        let mut without = heavy();
        switch(&mut without);
        assert_glides(
            &format!("block {index} off"),
            change(&heavy(), 0.4, set(without)),
        );
        assert_glides(
            &format!("block {index} on"),
            change(&without, 0.4, set(heavy())),
        );
    }
}

/// `params` with only block `keep` on.
fn only(keep: usize, params: ChainParams) -> ChainParams {
    let mut params = params;
    for (index, switch) in SWITCHES.iter().enumerate() {
        if index != keep {
            switch(&mut params);
        }
    }
    params
}

#[test]
fn settings_glide_without_clicks() {
    // Name, block, sine amplitude, change. The AGC's gain rises at first, on a
    // quiet sine.
    let cases: [(&str, usize, f64, Switch); 13] = [
        ("AGC maximum gain", 0, 0.01, |p| {
            p.gated_agc.max_gain_db = 0.0
        }),
        ("expander level", 1, 0.4, |p| p.stereo_expander.level = 0.0),
        ("bass EQ gain and frequency", 2, 0.4, |p| {
            p.bass_eq.gain_db = -12.0;
            p.bass_eq.frequency_hz = 250.0;
        }),
        ("bass EQ shape", 2, 0.4, |p| {
            p.bass_eq.shape = BassShape::Peak;
            p.bass_eq.gain_db = 12.0;
            p.bass_eq.frequency_hz = 250.0;
        }),
        ("5-band overdrive", 3, 0.2, |p| {
            p.five_band.overdrive_db = 12.0
        }),
        ("5-band crossovers", 3, 0.2, |p| {
            p.five_band.crossovers_hz = [60.0, 250.0, 1_000.0, 12_000.0];
        }),
        ("5-band gain, threshold, ratio", 3, 0.2, |p| {
            p.five_band.bands[1].gain_db = -12.0;
            p.five_band.bands[1].threshold_db = -50.0;
            p.five_band.bands[1].ratio = 20.0;
        }),
        ("5-band modes", 3, 0.2, |p| {
            p.five_band.bands[1].mode = BandMode::Limiter;
            p.five_band.bands[2].mode = BandMode::Expander;
        }),
        ("5-band links", 3, 0.2, |p| p.five_band.links = [true; 4]),
        ("dual-band crossover", 4, 0.2, |p| {
            p.dual_band.crossover_hz = 1_000.0
        }),
        ("dual-band mode", 4, 0.2, |p| {
            p.dual_band.bands[1].mode = BandMode::Limiter
        }),
        ("final ceiling", 5, 1.0, |p| p.final_stage.ceiling_db = -6.0),
        // Under the ceiling: the 5 ms of the limiter's delay crossfade away.
        ("final mode", 5, 0.5, |p| {
            p.final_stage.mode = FinalMode::Clipper
        }),
    ];
    // Nothing to glide, made at once or not: a change of mode moves a band's
    // gain with its attack or release, as a change of level does, and the
    // limiter's look-ahead ramps its gain to a new ceiling.
    let glide_only = ["5-band modes", "dual-band mode", "final ceiling"];
    for (name, block, amplitude, edit) in cases {
        let mut before = only(block, heavy());
        before.gated_agc.max_gain_db = 18.0;
        let mut after = before;
        edit(&mut after);
        assert_glides(name, change(&before, amplitude, |chain| chain.set(&after)));
        if !glide_only.contains(&name) {
            assert_clicks(
                name,
                change(&before, amplitude, |chain| chain.set_at_once(&after)),
            );
        }
    }
}

#[test]
fn the_meters_report_what_the_chain_does() {
    let mut chain = Chain::new(RATE);
    assert_eq!(chain.take_meters(), ChainMeters::default());
    chain.restart(&heavy());
    let mut audio = sine(1_000.0, 1.0, 2.0);
    run_chain(&mut chain, &mut audio);
    let meters = chain.take_meters();
    assert!(meters.agc_gain_db.is_some());
    assert!(meters.five_band_db.iter().any(|&g| g < -3.0), "{meters:?}");
    assert!(meters.final_stage_db < 0.0, "{meters:?}");
    chain.set(&all_off());
    run_chain(&mut chain, &mut sine(1_000.0, 1.0, 0.5));
    chain.take_meters();
    run_chain(&mut chain, &mut sine(1_000.0, 1.0, 0.1));
    assert_eq!(chain.take_meters(), ChainMeters::default());
}

#[test]
fn a_new_chain_takes_over_at_the_old_ones_agc_gain() {
    let params = heavy();
    let mut old = Chain::new(RATE);
    old.restart(&params);
    run_chain(
        &mut old,
        &mut programme(20.0).iter().map(|s| s * 0.3).collect::<Vec<_>>(),
    );
    let gain = old.agc_gain_db().unwrap_or_default();
    assert!(gain > 2.0, "{gain}");
    let mut new = Chain::new(RATE);
    new.restart(&params);
    new.start_agc_at(gain);
    let next: Vec<f32> = programme(1.0).iter().map(|s| s * 0.3).collect();
    let (mut a, mut b) = (next.clone(), next);
    run_chain(&mut old, &mut a);
    run_chain(&mut new, &mut b);
    let tail = |s: &[f32]| rms_db(&s[s.len() / 2..]);
    assert!(
        (tail(&a) - tail(&b)).abs() < 0.5,
        "{} {}",
        tail(&a),
        tail(&b)
    );
}

/// A measure, not a test: `cargo test -p heron-dsp --release -- --ignored
/// --nocapture agc::tests::cpu`.
#[test]
#[ignore = "a measurement"]
fn cpu() {
    let mut chain = Chain::new(RATE);
    chain.restart(&heavy());
    let mut audio = programme(60.0);
    let start = std::time::Instant::now();
    run_chain(&mut chain, &mut audio);
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "every block on: 60 s of audio in {seconds:.3} s, {:.2} % of one core",
        seconds / 60.0 * 100.0
    );
    for distortion in [50.0, 100.0, 1_000.0] {
        println!(
            "{distortion} Hz compressed by 10 dB: {:.3} % distortion",
            compressed_sine_distortion(distortion)
        );
    }
}
