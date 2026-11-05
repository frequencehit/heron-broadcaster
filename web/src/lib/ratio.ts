// SPDX-License-Identifier: AGPL-3.0-or-later

// The play ratio (docs/rotation.md#play-ratio): an exact quota of plays, in
// tenths. A station's counter goes from 10 down to 1, then back to 10; the
// track plays while the counter is in its first `ratio` steps.

import { number, t } from "../i18n";

/** A full ratio, and the counter's first step. */
export const FULL = 10;

/** The ratios a track can have, in tenths: 1 (0.1) to 10 (1). */
export const RATIOS = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] as const;

/** `0.2`, `1`, `0`, in the current language (`0,2` in French). */
export function ratioLabel(tenths: number): string {
  return number(tenths / FULL, { maximumFractionDigits: 1 });
}

/** Whether a track of `ratio` tenths plays with its counter at `counter`. */
export function plays(ratio: number, counter: number): boolean {
  return counter > FULL - Math.min(ratio, FULL);
}

/** A track's ratio as rotation sees it: 0 while it is disabled. */
export function effectiveRatio(track: { enabled: boolean; play_ratio: number }): number {
  return track.enabled ? track.play_ratio : 0;
}

export interface Cell {
  /** 10 down to 1. */
  step: number;
  plays: boolean;
  /** Where the counter is: the next time rotation picks the track. */
  next: boolean;
}

/** The 10 steps of a counter, from 10 down to 1. */
export function cells(ratio: number, counter: number): Cell[] {
  return Array.from({ length: FULL }, (_, index) => {
    const step = FULL - index;
    return { step, plays: plays(ratio, step), next: step === counter };
  });
}

/** What a ratio does: `Plays 2 times out of 10, then is held back 8 times.` */
export function ratioSummary(ratio: number): string {
  if (ratio <= 0) {
    return t("ratio.summaryOff");
  }
  if (ratio >= FULL) {
    return t("ratio.summaryFull");
  }
  return t("ratio.summary", { plays: ratio, held: FULL - ratio });
}

/** What happens the next time rotation picks the track, at `counter`. */
export function nextText(ratio: number, counter: number): string {
  if (ratio <= 0) {
    return t("ratio.nextOff");
  }
  if (plays(ratio, counter)) {
    return t("ratio.nextPlays");
  }
  return t("ratio.nextHeld", { count: counter });
}

/** The ratio filters of the track list, as the API's `ratio_min` and `ratio_max`. */
export const RATIO_FILTERS: Record<string, { ratio_min?: number; ratio_max: number }> = {
  below: { ratio_min: 1, ratio_max: FULL - 1 },
  low: { ratio_min: 1, ratio_max: 3 },
  off: { ratio_max: 0 },
};

export type RatioFilter = "below" | "low" | "off";

export function ratioFilter(value: string | null): { ratio_min?: number; ratio_max?: number } {
  return value && Object.hasOwn(RATIO_FILTERS, value) ? (RATIO_FILTERS[value] ?? {}) : {};
}
