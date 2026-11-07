// SPDX-License-Identifier: AGPL-3.0-or-later

// The Top 30 (docs/listener-interaction.md#top-30): movements, weeks and the
// show's programme as the pages show them.

import type { Schemas } from "../api";

export type Chart = Schemas["Chart"];
export type ChartEntry = Schemas["ChartEntry"];
export type ShowPlan = Schemas["ShowPlan"];
export type ProgrammeItem = Schemas["Item"];
export type Top30Settings = Schemas["Top30Settings"];

/** What the movement column shows: an arrow and the places, `=`, or a word. */
export function movement(entry: Pick<ChartEntry, "movement" | "rank" | "previous_rank">): {
  kind: ChartEntry["movement"];
  places: number | null;
} {
  const previous = entry.previous_rank ?? null;
  const places = previous === null ? null : Math.abs(previous - entry.rank);
  return { kind: entry.movement, places: entry.movement === "up" || entry.movement === "down" ? places : null };
}

/** The Monday of a week key (`2026-11-02`) as a UTC timestamp, and its Sunday. */
export function weekDays(week: string): { monday: number; sunday: number } {
  const monday = Date.parse(`${week}T00:00:00Z`);
  return { monday, sunday: monday + 6 * 86_400_000 };
}

/** The ranks left out, as a range: `30–28`, or a single rank. */
export function leftOutLabel(ranks: readonly number[]): string {
  if (ranks.length === 0) {
    return "";
  }
  const high = Math.max(...ranks);
  const low = Math.min(...ranks);
  return high === low ? String(high) : `${high}–${low}`;
}

/** The highest rank the show plays, and how many tracks. */
export function countdown(plan: Pick<ShowPlan, "items">): { tracks: number; from: number | null } {
  const ranks = plan.items
    .filter((item) => item.kind === "track" && !item.skipped)
    .map((item) => item.rank ?? 0);
  return { tracks: ranks.length, from: ranks.length ? Math.max(...ranks) : null };
}
