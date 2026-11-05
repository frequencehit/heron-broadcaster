// SPDX-License-Identifier: AGPL-3.0-or-later

// The master AGC chain: the range of each control (heron_core::agc checks
// the same), how a slider maps to a value, crossovers kept rising and
// apart as they are dragged, presets compared with the chain, and the
// settings as read then as the live WebSocket reports them changing. See
// docs/audio-engine.md#master-agc-chain.

import { useQuery } from "@tanstack/react-query";
import { type AgcPresetSettings, type AgcSettings, api, unwrap } from "../api";
import { useLive } from "./live";
import { here } from "./station";

export const AGC_KEY = ["settings", "agc"] as const;
export const AGC_PRESETS_KEY = ["agc-presets"];

export interface Range {
  min: number;
  max: number;
  /** Values are rounded to this. */
  step: number;
  unit: string;
  /** The slider moves on a logarithmic scale (frequencies, times, ratios). */
  log?: boolean;
}

export const RANGES = {
  gateLufs: { min: -70, max: -20, step: 1, unit: "LUFS" },
  maxGainDb: { min: 0, max: 18, step: 0.5, unit: "dB" },
  percent: { min: 0, max: 100, step: 1, unit: "%" },
  levelDb: { min: -60, max: 0, step: 0.5, unit: "dB" },
  bassFrequencyHz: { min: 40, max: 250, step: 1, unit: "Hz", log: true },
  gainDb: { min: -12, max: 12, step: 0.5, unit: "dB" },
  overdriveDb: { min: 0, max: 12, step: 0.5, unit: "dB" },
  crossoverHz: { min: 40, max: 16_000, step: 1, unit: "Hz", log: true },
  dualCrossoverHz: { min: 60, max: 2_000, step: 1, unit: "Hz", log: true },
  ratio: { min: 1, max: 20, step: 0.1, unit: ":1", log: true },
  attackMs: { min: 0.5, max: 200, step: 0.5, unit: "ms", log: true },
  releaseMs: { min: 10, max: 2_000, step: 5, unit: "ms", log: true },
  holdMs: { min: 0, max: 500, step: 5, unit: "ms" },
  ceilingDb: { min: -6, max: 0, step: 0.1, unit: "dB" },
  finalReleaseMs: { min: 10, max: 1_000, step: 5, unit: "ms", log: true },
} satisfies Record<string, Range>;

/** Crossovers are at least a third of an octave apart (AGC_MIN_CROSSOVER_RATIO). */
export const MIN_CROSSOVER_RATIO = 1.259921;

/** Positions of a slider: fine enough for 1 Hz steps at the low end. */
export const SLIDER_STEPS = 1_000;

function decimals(step: number): number {
  const text = String(step);
  const dot = text.indexOf(".");
  return dot < 0 ? 0 : text.length - dot - 1;
}

/** A value within the range, rounded to its step. */
export function snap(range: Range, value: number): number {
  const clamped = Math.min(range.max, Math.max(range.min, value));
  const rounded = Math.round(clamped / range.step) * range.step;
  return Number(Math.min(range.max, Math.max(range.min, rounded)).toFixed(decimals(range.step)));
}

/** Where a value sits on its slider, 0 to SLIDER_STEPS. */
export function toPosition(range: Range, value: number): number {
  const clamped = Math.min(range.max, Math.max(range.min, value));
  const fraction = range.log
    ? Math.log(clamped / range.min) / Math.log(range.max / range.min)
    : (clamped - range.min) / (range.max - range.min);
  return Math.round(fraction * SLIDER_STEPS);
}

/** The value at a slider position. */
export function fromPosition(range: Range, position: number): number {
  const fraction = Math.min(1, Math.max(0, position / SLIDER_STEPS));
  const value = range.log
    ? range.min * (range.max / range.min) ** fraction
    : range.min + fraction * (range.max - range.min);
  return snap(range, value);
}

/** Arrow keys move a slider by this many positions (1 % of its travel). */
const KEY_POSITIONS = 10;

/**
 * Where an arrow key takes a value: 1 % of the slider's travel, and at
 * least one step, so a value never rounds back to where it was.
 */
export function keyStep(range: Range, value: number, direction: 1 | -1): number {
  const moved = fromPosition(range, toPosition(range, value) + direction * KEY_POSITIONS);
  return moved === value ? snap(range, value + direction * range.step) : moved;
}

/**
 * The crossovers with one moved, kept rising and a third of an octave from
 * their neighbours: the slider stops where the engine would refuse it.
 */
export function withCrossover(crossovers: number[], index: number, value: number, range: Range): number[] {
  const below = index > 0 ? Math.ceil(crossovers[index - 1]! * MIN_CROSSOVER_RATIO) : range.min;
  const above = index < crossovers.length - 1 ? Math.floor(crossovers[index + 1]! / MIN_CROSSOVER_RATIO) : range.max;
  const next = Math.min(above, Math.max(below, Math.round(value)));
  return crossovers.map((frequency, i) => (i === index ? next : frequency));
}

/** "80 Hz", "1.6 kHz", "16 kHz". */
export function formatHz(frequency: number): string {
  if (frequency >= 1_000) {
    return `${Number((frequency / 1_000).toFixed(frequency >= 10_000 ? 0 : 1))} kHz`;
  }
  return `${Math.round(frequency)} Hz`;
}

/** What each band of a processor covers: `[from, to]`, open at the ends. */
export function bandRanges(crossovers: number[]): [number | null, number | null][] {
  return [null, ...crossovers].map((from, i) => [from, crossovers[i] ?? null]);
}

/** "+3.5", "0.0", "-12.0". */
export function formatDb(db: number, digits = 1): string {
  const text = db.toFixed(digits);
  return db > 0 && Number(text) !== 0 ? `+${text}` : text.replace(/^-(0\.?0*)$/, "$1");
}

/** A preset's blocks: the chain without `bypass_all`. */
export function blocksOf(settings: AgcSettings): AgcPresetSettings {
  const { bypass_all: _, ...blocks } = settings;
  return blocks;
}

/** Loading a preset: its blocks, the chain's bypass kept. */
export function withBlocks(settings: AgcSettings, blocks: AgcPresetSettings): AgcSettings {
  return { ...blocks, bypass_all: settings.bypass_all };
}

/** Equal values, whatever the order of the keys. */
export function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) {
    return true;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((value, i) => sameValue(value, b[i]));
  }
  if (a && b && typeof a === "object" && typeof b === "object" && !Array.isArray(a) && !Array.isArray(b)) {
    const keys = Object.keys(a);
    return (
      keys.length === Object.keys(b).length &&
      keys.every((key) => sameValue((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]))
    );
  }
  return false;
}

/** The preset whose blocks are the chain's, if any. */
export function presetMatching<T extends { settings: AgcPresetSettings }>(
  presets: T[] | undefined,
  settings: AgcSettings,
): T | undefined {
  const blocks = blocksOf(settings);
  return presets?.find((preset) => sameValue(preset.settings, blocks));
}

/** The blocks that are on, in the chain's order. */
export const BLOCKS = ["gated_agc", "stereo_expander", "bass_eq", "five_band", "dual_band", "final_stage"] as const;
export type Block = (typeof BLOCKS)[number];

/** Some block changes the sound (heron_core::AgcSettings::is_processing). */
export function isProcessing(settings: AgcSettings | null | undefined): boolean {
  return !!settings && !settings.bypass_all && BLOCKS.some((block) => settings[block].enabled);
}

/** Gain reduction meters show 0 to this, in dB. */
export const REDUCTION_RANGE_DB = 24;

/** How far a gain reduction (0 or less) fills its meter, 0 to 1. */
export function reductionScale(db: number | null | undefined): number {
  if (db === null || db === undefined || !Number.isFinite(db)) {
    return 0;
  }
  return Math.min(1, Math.max(0, -db / REDUCTION_RANGE_DB));
}

/**
 * The AGC chain: as read, then as the live WebSocket reports it changing.
 * Read again on reconnection.
 */
export function useAgcSettings(): { data: AgcSettings | undefined; error: unknown } {
  const connected = useLive((live) => live.connected);
  const live = useLive((live) => live.agc);
  const query = useQuery({
    queryKey: [...AGC_KEY, connected],
    // Read again on every reconnection: the live messages missed meanwhile
    // are not sent again.
    staleTime: 0,
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/agc", { params: { path: here() } })),
  });
  return { data: live ?? query.data, error: query.error };
}
