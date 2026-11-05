// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { AgcSettings } from "../api";
import {
  MIN_CROSSOVER_RATIO,
  RANGES,
  SLIDER_STEPS,
  bandRanges,
  blocksOf,
  formatDb,
  formatHz,
  fromPosition,
  isProcessing,
  keyStep,
  presetMatching,
  reductionScale,
  sameValue,
  snap,
  toPosition,
  withBlocks,
  withCrossover,
} from "./agc";

const band = {
  mode: "compressor" as const,
  threshold_db: -20,
  ratio: 3,
  attack_ms: 20,
  release_ms: 250,
  hold_ms: 0,
  gain_db: 0,
};

/** heron_core::AgcSettings::default(). */
function defaults(): AgcSettings {
  return {
    bypass_all: true,
    gated_agc: { enabled: false, gate_lufs: -45, max_gain_db: 6 },
    stereo_expander: { enabled: false, level: 50, depth: 50, threshold_db: -40 },
    bass_eq: { enabled: false, shape: "shelf", frequency_hz: 100, gain_db: 0 },
    five_band: {
      enabled: false,
      overdrive_db: 0,
      crossovers_hz: [100, 400, 1_600, 6_400],
      links: [false, false, false, false],
      bands: [band, band, band, band, band],
    },
    dual_band: { enabled: false, crossover_hz: 200, link: false, bands: [band, band] },
    final_stage: { enabled: true, mode: "limiter", ceiling_db: -1, release_ms: 100 },
  };
}

describe("sliders", () => {
  it("snap values to their range and step", () => {
    expect(snap(RANGES.ceilingDb, -0.33)).toBe(-0.3);
    expect(snap(RANGES.ceilingDb, 2)).toBe(0);
    expect(snap(RANGES.attackMs, 0.1)).toBe(0.5);
    expect(snap(RANGES.attackMs, 12.3)).toBe(12.5);
    expect(snap(RANGES.ratio, 2.26)).toBe(2.3);
    expect(String(snap(RANGES.ratio, 0.1 + 0.2 + 2))).toBe("2.3");
  });

  it("go from end to end, linearly or on a log scale", () => {
    for (const range of Object.values(RANGES)) {
      expect(fromPosition(range, 0)).toBe(range.min);
      expect(fromPosition(range, SLIDER_STEPS)).toBe(range.max);
      expect(toPosition(range, range.min)).toBe(0);
      expect(toPosition(range, range.max)).toBe(SLIDER_STEPS);
    }
    // The middle of a frequency slider is the geometric mean: 800 Hz.
    expect(fromPosition(RANGES.crossoverHz, SLIDER_STEPS / 2)).toBe(800);
    expect(fromPosition(RANGES.gainDb, SLIDER_STEPS / 2)).toBe(0);
  });

  it("move with the arrow keys, at least a step at a time", () => {
    expect(keyStep(RANGES.ceilingDb, -1, -1)).toBe(-1.1);
    expect(keyStep(RANGES.ceilingDb, -1, 1)).toBe(-0.9);
    expect(keyStep(RANGES.ceilingDb, 0, 1)).toBe(0);
    expect(keyStep(RANGES.crossoverHz, 1_000, 1)).toBe(1_060);
    expect(keyStep(RANGES.crossoverHz, 40, -1)).toBe(40);
    expect(keyStep(RANGES.gainDb, 0, 1)).toBe(0.5);
    for (const range of Object.values(RANGES)) {
      const middle = fromPosition(range, SLIDER_STEPS / 2);
      expect(keyStep(range, middle, 1)).toBeGreaterThan(middle);
      expect(keyStep(range, middle, -1)).toBeLessThan(middle);
    }
  });

  it("give back the value they show", () => {
    for (const value of [40, 100, 400, 1_600, 6_400, 16_000]) {
      const back = fromPosition(RANGES.crossoverHz, toPosition(RANGES.crossoverHz, value));
      expect(Math.abs(back - value) / value).toBeLessThan(0.005);
    }
    expect(fromPosition(RANGES.gateLufs, toPosition(RANGES.gateLufs, -45))).toBe(-45);
  });
});

describe("crossovers", () => {
  it("stay rising and a third of an octave apart", () => {
    const crossovers = [100, 400, 1_600, 6_400];
    expect(withCrossover(crossovers, 1, 250, RANGES.crossoverHz)).toEqual([100, 250, 1_600, 6_400]);
    const pushed = withCrossover(crossovers, 1, 50, RANGES.crossoverHz);
    expect(pushed[1]).toBe(126);
    expect(pushed[1]! >= 100 * MIN_CROSSOVER_RATIO).toBe(true);
    const up = withCrossover(crossovers, 2, 99_999, RANGES.crossoverHz);
    expect(up[2]).toBe(5_079);
    expect(up[3]! >= up[2]! * MIN_CROSSOVER_RATIO).toBe(true);
    expect(withCrossover(crossovers, 0, 10, RANGES.crossoverHz)[0]).toBe(40);
    expect(withCrossover(crossovers, 3, 20_000, RANGES.crossoverHz)[3]).toBe(16_000);
    expect(withCrossover([200], 0, 5_000, RANGES.dualCrossoverHz)).toEqual([2_000]);
  });

  it("name what each band covers", () => {
    expect(bandRanges([100, 400])).toEqual([
      [null, 100],
      [100, 400],
      [400, null],
    ]);
    expect(formatHz(80)).toBe("80 Hz");
    expect(formatHz(1_600)).toBe("1.6 kHz");
    expect(formatHz(1_000)).toBe("1 kHz");
    expect(formatHz(16_000)).toBe("16 kHz");
  });
});

describe("presets", () => {
  it("keep the blocks, not the bypass", () => {
    const settings = { ...defaults(), bypass_all: false };
    const blocks = blocksOf(settings);
    expect("bypass_all" in blocks).toBe(false);
    expect(withBlocks(defaults(), blocks)).toEqual(defaults());
    expect(withBlocks(settings, blocks).bypass_all).toBe(false);
  });

  it("are found by their blocks, whatever the key order", () => {
    const settings = defaults();
    const reordered = Object.fromEntries(
      Object.entries(blocksOf(settings))
        .reverse()
        .map(([key, value]) => [key, Object.fromEntries(Object.entries(value).reverse())]),
    );
    const presets = [
      { id: 1, name: "Other", settings: { ...blocksOf(settings), bass_eq: { ...settings.bass_eq, gain_db: 3 } } },
      { id: 2, name: "Same", settings: { ...blocksOf(settings), final_stage: { ...settings.final_stage } } },
    ];
    expect(presetMatching(presets, settings)?.id).toBe(2);
    expect(presetMatching(presets, { ...settings, bypass_all: false })?.id).toBe(2);
    expect(presetMatching(undefined, settings)).toBeUndefined();
    expect(sameValue(reordered, blocksOf(settings))).toBe(true);
    expect(sameValue([1, 2], [1, 2, 3])).toBe(false);
    expect(sameValue({ a: 1 }, { b: 1 })).toBe(false);
    expect(sameValue(null, {})).toBe(false);
  });
});

describe("the chain", () => {
  it("processes when on with a block on", () => {
    const settings = defaults();
    expect(isProcessing(settings)).toBe(false);
    expect(isProcessing({ ...settings, bypass_all: false })).toBe(true);
    expect(
      isProcessing({ ...settings, bypass_all: false, final_stage: { ...settings.final_stage, enabled: false } }),
    ).toBe(false);
    expect(isProcessing(null)).toBe(false);
  });

  it("shows gain reductions and gains", () => {
    expect(reductionScale(0)).toBe(0);
    expect(reductionScale(-12)).toBe(0.5);
    expect(reductionScale(-40)).toBe(1);
    expect(reductionScale(2)).toBe(0);
    expect(reductionScale(undefined)).toBe(0);
    expect(reductionScale(Number.NaN)).toBe(0);
    expect(formatDb(3.25)).toBe("+3.3");
    expect(formatDb(0)).toBe("0.0");
    expect(formatDb(-0.04)).toBe("0.0");
    expect(formatDb(-4.2)).toBe("-4.2");
  });
});
