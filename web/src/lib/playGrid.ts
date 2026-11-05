// SPDX-License-Identifier: AGPL-3.0-or-later

// The track page's grids of plays (docs/web.md#plays-by-day-and-hour): the
// last 30 days as weeks, Monday to Sunday, and the hours of the week. The
// API gives the days and hours with their plays, at their local time.

import type { Schemas } from "../api";
import { locale } from "../i18n";

export type PlayGrid = Schemas["PlayGrid"];
export type GridDay = Schemas["GridDay"];
export type GridHour = Schemas["GridHour"];
export type GridPlay = Schemas["GridPlay"];

/** A day in a week row: in the 30 days (with its plays) or greyed out. */
export interface WeekCell {
  date: string;
  day: GridDay | null;
}

export interface Week {
  /** The Monday it starts on. */
  monday: string;
  cells: WeekCell[];
  /** The plays of its days in the 30 days. */
  total: number;
}

const DAY_MS = 86_400_000;

/** `2026-10-12` as UTC midnight, for date arithmetic and formatting. */
function utc(date: string): number {
  const [year = 1970, month = 1, day = 1] = date.split("-").map(Number);
  return Date.UTC(year, month - 1, day);
}

function iso(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/** The days (oldest first) as weeks, Monday to Sunday, with the days around them greyed out. */
export function weeks(days: GridDay[]): Week[] {
  const first = days[0];
  const last = days.at(-1);
  if (!first || !last) {
    return [];
  }
  const byDate = new Map(days.map((day) => [day.date, day]));
  const start = utc(first.date) - (first.weekday - 1) * DAY_MS;
  const end = utc(last.date) + (7 - last.weekday) * DAY_MS;
  const out: Week[] = [];
  for (let monday = start; monday <= end; monday += 7 * DAY_MS) {
    const cells = Array.from({ length: 7 }, (_, index) => {
      const date = iso(monday + index * DAY_MS);
      return { date, day: byDate.get(date) ?? null };
    });
    const total = cells.reduce((sum, cell) => sum + (cell.day?.plays.length ?? 0), 0);
    out.push({ monday: iso(monday), cells, total });
  }
  return out;
}

/** 0 for no plays, then 1 to 4, darker the more plays; up to 4 plays each step is one play. */
export function shade(count: number, most: number): number {
  if (count <= 0) {
    return 0;
  }
  return Math.min(4, Math.ceil((4 * count) / Math.max(most, 4)));
}

/** The hours with plays, by `weekday-hour`. */
export function hourCells(hours: GridHour[]): Map<string, GridHour> {
  return new Map(hours.map((cell) => [cellKey(cell.weekday, cell.hour), cell]));
}

export function cellKey(weekday: number, hour: number): string {
  return `${weekday}-${hour}`;
}

/** The most plays in a day, or in an hour of the week. */
export function most(cells: { plays: unknown[] }[]): number {
  return cells.reduce((max, cell) => Math.max(max, cell.plays.length), 0);
}

/** `12 Oct`, or `Mon 12 Oct` with the weekday, in the current language. */
export function dateLabel(date: string, withWeekday = false): string {
  return new Intl.DateTimeFormat(locale(), {
    day: "numeric",
    month: "short",
    weekday: withWeekday ? "short" : undefined,
    timeZone: "UTC",
  }).format(utc(date));
}

/** The history at a day, for a track. */
export function historyHref(trackId: number, day: GridDay): string {
  return `/history?track=${trackId}&after=${day.start_unix_ms}&before=${day.end_unix_ms}`;
}

/** The history's filter from its address: a track over a span of time. */
export interface HistoryFilter {
  trackId: number;
  after?: number | undefined;
  before?: number | undefined;
}

export function historyFilter(search: URLSearchParams): HistoryFilter | null {
  const trackId = Number(search.get("track"));
  if (!Number.isInteger(trackId) || trackId <= 0) {
    return null;
  }
  const time = (name: string) => {
    const value = Number(search.get(name));
    return search.has(name) && Number.isFinite(value) ? value : undefined;
  };
  return { trackId, after: time("after"), before: time("before") };
}
