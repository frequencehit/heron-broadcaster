// SPDX-License-Identifier: AGPL-3.0-or-later

// The deck EQ: its bands, the response curve the engine's filters have
// (shelves and peaking bands of the RBJ cookbook, their gains solved from
// the sliders, as in heron_dsp::eq; eq.fixture.json keeps both equal), and how slider
// moves are sent: the latest value, one request at a time, at most every
// 100 ms. See docs/audio-engine.md#deck-eq.

import { useQuery } from "@tanstack/react-query";
import { type DeckEq, type EqSettings, api, unwrap } from "../api";
import { useLive } from "./live";
import { here } from "./station";

/** Frequency of each band, in Hz: where the curve meets its slider. */
export const FREQUENCIES = [60, 170, 310, 600, 1_000, 3_000, 6_000, 12_000, 14_000, 16_000];
export const MAX_GAIN_DB = 12;
/** Filter gains solved from the sliders stay within this. */
const MAX_FILTER_GAIN_DB = 24;
const SHELF_Q = Math.SQRT1_2;
/** The shelves' midpoints, an octave inside their bands: on their plateau at 60 Hz and 16 kHz. */
const SHELF_MIDPOINTS = { low_shelf: 120, high_shelf: 8_000 };
const SOLVE_ROUNDS = 4;
const SAMPLE_RATE = 48_000;
const BANDS = FREQUENCIES.length;

type Shape = "low_shelf" | "peaking" | "high_shelf";

function shape(band: number): Shape {
  if (band === 0) {
    return "low_shelf";
  }
  return band === BANDS - 1 ? "high_shelf" : "peaking";
}

/** A peaking band is as wide as the distances to its neighbours. */
export function bandQ(band: number): number {
  if (shape(band) !== "peaking") {
    return SHELF_Q;
  }
  const below = Math.log2(FREQUENCIES[band]! / FREQUENCIES[band - 1]!);
  const above = Math.log2(FREQUENCIES[band + 1]! / FREQUENCIES[band]!);
  const ratio = 2 ** ((below + above) / 2);
  return Math.sqrt(ratio) / (ratio - 1);
}

/** Gain of one band's filter at `frequency`, in dB (the cookbook's designs). */
function bandDb(band: number, gainDb: number, frequency: number): number {
  const a = 10 ** (gainDb / 40);
  const kind = shape(band);
  const designed = kind === "peaking" ? FREQUENCIES[band]! : SHELF_MIDPOINTS[kind];
  const w0 = (2 * Math.PI * designed) / SAMPLE_RATE;
  const alpha = Math.sin(w0) / (2 * bandQ(band));
  const cos = Math.cos(w0);
  let b: [number, number, number];
  let d: [number, number, number];
  if (kind === "peaking") {
    b = [1 + alpha * a, -2 * cos, 1 - alpha * a];
    d = [1 + alpha / a, -2 * cos, 1 - alpha / a];
  } else {
    const root = 2 * Math.sqrt(a) * alpha;
    const s = kind === "high_shelf" ? -1 : 1;
    b = [
      a * (a + 1 - s * (a - 1) * cos + root),
      s * 2 * a * (a - 1 - s * (a + 1) * cos),
      a * (a + 1 - s * (a - 1) * cos - root),
    ];
    d = [a + 1 + s * (a - 1) * cos + root, -s * 2 * (a - 1 + s * (a + 1) * cos), a + 1 + s * (a - 1) * cos - root];
  }
  const w = (2 * Math.PI * frequency) / SAMPLE_RATE;
  const magnitude = ([c0, c1, c2]: [number, number, number]) => {
    const re = c0 + c1 * Math.cos(w) + c2 * Math.cos(2 * w);
    const im = -(c1 * Math.sin(w) + c2 * Math.sin(2 * w));
    return re * re + im * im;
  };
  return 10 * Math.log10(magnitude(b) / magnitude(d));
}

/** Gain at `frequency` of the filters at these gains, in dB. */
export function filtersResponseDb(filterGains: number[], frequency: number): number {
  return filterGains.reduce((sum, gain, band) => sum + bandDb(band, gain, frequency), 0);
}

/**
 * The filter gains whose summed response meets each slider at its band's
 * centre, as the engine solves them (heron_dsp::eq::filter_gains).
 */
export function filterGains(sliders: number[]): number[] {
  const target = sliders.map((gain) => Math.max(-MAX_GAIN_DB, Math.min(MAX_GAIN_DB, gain)));
  let gains = [...target];
  for (let round = 0; round < SOLVE_ROUNDS; round++) {
    const matrix = FREQUENCIES.map(() => new Array<number>(BANDS).fill(0));
    gains.forEach((gain, band) => {
      const probe = Math.abs(gain) > 0.5 ? gain : 1;
      FREQUENCIES.forEach((centre, row) => {
        matrix[row]![band] = bandDb(band, probe, centre) / probe;
      });
    });
    const error = FREQUENCIES.map((centre, band) => target[band]! - filtersResponseDb(gains, centre));
    const step = solve(matrix, error);
    if (!step) {
      break;
    }
    gains = gains.map((gain, band) =>
      Math.max(-MAX_FILTER_GAIN_DB, Math.min(MAX_FILTER_GAIN_DB, gain + step[band]!)),
    );
  }
  return gains;
}

/** Solves `matrix · x = vector` (Gauss–Jordan with partial pivoting). */
function solve(matrix: number[][], vector: number[]): number[] | null {
  const m = matrix.map((row) => [...row]);
  const v = [...vector];
  for (let column = 0; column < BANDS; column++) {
    let pivot = column;
    for (let row = column + 1; row < BANDS; row++) {
      if (Math.abs(m[row]![column]!) > Math.abs(m[pivot]![column]!)) {
        pivot = row;
      }
    }
    if (Math.abs(m[pivot]![column]!) < 1e-12) {
      return null;
    }
    [m[column], m[pivot]] = [m[pivot]!, m[column]!];
    [v[column], v[pivot]] = [v[pivot]!, v[column]!];
    for (let row = 0; row < BANDS; row++) {
      if (row !== column) {
        const factor = m[row]![column]! / m[column]![column]!;
        m[row] = m[row]!.map((value, i) => value - factor * m[column]![i]!);
        v[row] = v[row]! - factor * v[column]!;
      }
    }
  }
  return v.map((value, row) => value / m[row]![row]!);
}

/** Gain of the EQ at `frequency` for these sliders, in dB. */
export function responseDb(sliders: number[], frequency: number): number {
  return filtersResponseDb(filterGains(sliders), frequency);
}

export const EQ_KEY = ["settings", "eq"] as const;

/**
 * The deck EQ: as read, then as the live WebSocket reports it changing.
 * Read again on reconnection.
 */
export function useEqSettings(): { data: EqSettings | undefined; error: unknown } {
  const connected = useLive((live) => live.connected);
  const live = useLive((live) => live.eq);
  const query = useQuery({
    queryKey: [...EQ_KEY, connected],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/eq", { params: { path: here() } })),
  });
  return { data: live ?? query.data, error: query.error };
}

/** "+3.5", "0.0", "-12.0". */
export function formatGain(gain: number): string {
  return gain > 0 ? `+${gain.toFixed(1)}` : gain.toFixed(1);
}

/** The EQ with one band's gain changed, rounded and within ±12 dB. */
export function withGain(eq: DeckEq, band: number, gain: number): DeckEq {
  return { ...eq, gains_db: eq.gains_db.map((g, i) => (i === band ? roundGain(gain) : g)) };
}

/** What an arrow key does to a slider: half a decibel up or down. */
export function arrowStep(key: string): number | undefined {
  return { ArrowUp: 0.5, ArrowRight: 0.5, ArrowDown: -0.5, ArrowLeft: -0.5 }[key];
}

/** The preset with exactly these gains, if any. */
export function presetWithGains<T extends { gains_db: number[] }>(presets: T[] | undefined, gains: number[]): T | undefined {
  return presets?.find(
    (preset) => preset.gains_db.length === gains.length && preset.gains_db.every((gain, i) => gain === gains[i]),
  );
}

// The curve: 20 Hz to 20 kHz on a log scale, ±15 dB: a band's ±12 and
// some of what neighbours add up to.
export const CURVE_WIDTH = 600;
export const CURVE_HEIGHT = 140;
const CURVE_RANGE_DB = 15;

/** Where a frequency is across the curve. */
export function curveX(frequency: number): number {
  return (Math.log10(frequency / 20) / 3) * CURVE_WIDTH;
}

/** Where a gain is up the curve, clamped to its range. */
export function curveY(db: number): number {
  const clamped = Math.max(-CURVE_RANGE_DB, Math.min(CURVE_RANGE_DB, db));
  return CURVE_HEIGHT / 2 - (clamped / CURVE_RANGE_DB) * (CURVE_HEIGHT / 2);
}

/** The SVG path of the EQ's curve for these sliders, in `steps` segments. */
export function curvePath(sliders: number[], steps: number, decimals: number): string {
  const filters = filterGains(sliders);
  const points: string[] = [];
  for (let step = 0; step <= steps; step++) {
    const frequency = 20 * 1000 ** (step / steps);
    points.push(`${curveX(frequency).toFixed(decimals)},${curveY(filtersResponseDb(filters, frequency)).toFixed(decimals)}`);
  }
  return `M${points.join("L")}`;
}

/** "60", "1k", "12k". */
export function bandLabel(frequency: number): string {
  return frequency >= 1_000 ? `${frequency / 1_000}k` : String(frequency);
}

/** The EQ changes the sound: on, with a band away from 0 dB. */
export function isShaping(eq: DeckEq | undefined | null): boolean {
  return !!eq?.enabled && eq.gains_db.some((gain) => gain !== 0);
}

/** A gain within ±12 dB, to the nearest tenth of a decibel. */
export function roundGain(gain: number): number {
  return Math.max(-MAX_GAIN_DB, Math.min(MAX_GAIN_DB, Math.round(gain * 10) / 10));
}

/**
 * Sends values as they change: one request at a time, at most every
 * `intervalMs`, and only the latest value: those in between are skipped.
 */
export function latestSender<T>(send: (value: T) => Promise<unknown>, intervalMs = 100) {
  let pending: { value: T } | null = null;
  let busy = false;
  let lastSent = Number.NEGATIVE_INFINITY;
  let onError: (error: unknown) => void = () => {};

  const pump = async () => {
    if (busy || !pending) {
      return;
    }
    busy = true;
    const wait = lastSent + intervalMs - Date.now();
    if (wait > 0) {
      await new Promise((resolve) => setTimeout(resolve, wait));
    }
    const next = pending;
    pending = null;
    lastSent = Date.now();
    try {
      if (next) {
        await send(next.value);
      }
    } catch (error) {
      onError(error);
    } finally {
      busy = false;
    }
    void pump();
  };

  return {
    push(value: T) {
      pending = { value };
      void pump();
    },
    /** Waiting to send, or sending. */
    get active() {
      return busy || pending !== null;
    },
    set onError(handler: (error: unknown) => void) {
      onError = handler;
    },
  };
}
