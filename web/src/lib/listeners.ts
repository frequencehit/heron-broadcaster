// SPDX-License-Identifier: AGPL-3.0-or-later

// Listener statistics: the current counts, and the days of a period in
// station time (see docs/statistics.md).

import { useModule } from "./modules";
import { useQuery } from "@tanstack/react-query";
import { type Schemas, api, unwrap } from "../api";
import { locale, number, t } from "../i18n";
import { here, stationUrl } from "./station";

/** Each output's listeners at the last minute; read again every half minute. */
export function useListenersNow() {
  // Nothing to read while listener statistics are off (docs/modules.md).
  const on = useModule("listener_statistics");
  return useQuery({
    // Per station: another station's count is not shown while this one's loads.
    queryKey: ["listeners", "now", here().station_id],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/listeners/now", { params: { path: here() } })),
    refetchInterval: 30_000,
    enabled: on,
  });
}

/** `YYYY-MM-DD` of a Unix time in ms, in `timeZone` (the browser's when missing). */
export function dayOf(unixMs: number, timeZone?: string): string {
  // en-CA writes dates as YYYY-MM-DD.
  return new Intl.DateTimeFormat("en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).format(
    new Date(unixMs),
  );
}

/** The day `count` days after `day` (before when negative). */
export function addDays(day: string, count: number): string {
  const [y, m, d] = day.split("-").map(Number);
  const date = new Date(Date.UTC(y ?? 1970, (m ?? 1) - 1, (d ?? 1) + count));
  return date.toISOString().slice(0, 10);
}

/** How far `timeZone` is ahead of UTC at a time, in ms. */
function offset(unixMs: number, timeZone?: string): number {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", {
      timeZone,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    })
      .formatToParts(new Date(unixMs))
      .map((part) => [part.type, Number(part.value)]),
  );
  const local = Date.UTC(parts.year ?? 1970, (parts.month ?? 1) - 1, parts.day ?? 1, parts.hour ?? 0, parts.minute ?? 0, parts.second ?? 0);
  return local - Math.floor(unixMs / 1000) * 1000;
}

/** When a day starts in `timeZone`, in Unix ms. */
export function dayStart(day: string, timeZone?: string): number {
  const [y, m, d] = day.split("-").map(Number);
  const utc = Date.UTC(y ?? 1970, (m ?? 1) - 1, d ?? 1);
  // Twice: the offset at midnight may differ from the offset at the guess.
  const guess = utc - offset(utc, timeZone);
  return utc - offset(guess, timeZone);
}

/** A ratio as a percentage, `12.5 %`. */
export function percent(ratio: number | null | undefined, language?: string): string {
  if (ratio === null || ratio === undefined || !Number.isFinite(ratio)) {
    return "–";
  }
  return new Intl.NumberFormat(language, { style: "percent", maximumFractionDigits: 1 }).format(ratio);
}

export type Grouping = "day" | "week" | "month";
export type Measure = "listening_hours" | "peak" | "average" | "distinct_listeners";
type PeriodTotals = Schemas["PeriodTotals"];

/** A number with at most `digits` decimals; `–` when missing. */
export function decimal(value: number | null | undefined, digits = 1): string {
  return value === null || value === undefined ? "–" : number(value, { maximumFractionDigits: digits });
}

/** Listening hours: hundredths while they are few. */
export function listeningHours(value: number | null | undefined): string {
  return decimal(value, value !== null && value !== undefined && value < 10 ? 2 : 1);
}

const SHORT_DAY: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };

/** A day (`YYYY-MM-DD`) written short, `19 Oct`. */
export function shortDay(day: string, options: Intl.DateTimeFormatOptions = SHORT_DAY): string {
  const [y, m, d] = day.split("-").map(Number);
  return new Intl.DateTimeFormat(locale(), { ...options, timeZone: "UTC" }).format(
    new Date(Date.UTC(y ?? 1970, (m ?? 1) - 1, d ?? 1)),
  );
}

/** A period by its first day: a day, `Week of 19 Oct`, `October 2026`. */
export function periodLabel(day: string, grouping: Grouping): string {
  switch (grouping) {
    case "week":
      return t("statistics.weekOf", { day: shortDay(day) });
    case "month":
      return shortDay(day, { month: "long", year: "numeric" });
    default:
      return shortDay(day, { weekday: "short", month: "short", day: "numeric" });
  }
}

/** The label under a period's bar. */
export function barLabel(day: string, grouping: Grouping): string {
  return grouping === "month" ? shortDay(day, { month: "short" }) : shortDay(day);
}

/** One measure of a period, written. */
export function measureText(row: PeriodTotals, measure: Measure): string {
  switch (measure) {
    case "peak":
      return number(row.peak);
    case "average":
      return decimal(row.average);
    case "distinct_listeners":
      return decimal(row.distinct_listeners);
    default:
      return listeningHours(row.listening_hours);
  }
}

/** `08h`. */
export function hourLabel(hour: number): string {
  return `${String(hour).padStart(2, "0")}h`;
}

/** Where to download a table in CSV; parameters left undefined are left out. */
export function csvUrl(path: string, params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams({ format: "csv" });
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) {
      search.set(key, String(value));
    }
  }
  return stationUrl(`listeners/${path}?${search.toString()}`);
}

/** The statistics settings sent with an output: a password left empty keeps the stored one. */
export function statisticsInput(
  user: string,
  password: string,
  remove: boolean,
): { admin_username: string | null; admin_password?: string | null } {
  const input: { admin_username: string | null; admin_password?: string | null } = {
    admin_username: user.trim() || null,
  };
  if (remove) {
    input.admin_password = null;
  } else if (password) {
    input.admin_password = password;
  }
  return input;
}

/** The output and server the Statistics page is limited to (`?output=`, `?server=`). */
export interface Filter {
  output: number | undefined;
  server: number | undefined;
}

function id(value: string | null): number | undefined {
  const parsed = Number(value);
  return value && Number.isInteger(parsed) && parsed > 0 ? parsed : undefined;
}

/** The filter of the page's address. */
export function filterOf(search: URLSearchParams): Filter {
  return { output: id(search.get("output")), server: id(search.get("server")) };
}

/** The filter as query parameters of the statistics routes. */
export function filterParams(filter: Filter): { output_id: number | undefined; server_id: number | undefined } {
  return { output_id: filter.output, server_id: filter.server };
}

/** The change from the previous period, as a ratio; `null` when it cannot be told. */
export function change(current: number | null | undefined, previous: number | null | undefined): number | null {
  if (current === null || current === undefined || previous === null || previous === undefined || previous <= 0) {
    return null;
  }
  return (current - previous) / previous;
}

/** A change written with its sign, `+12.5 %`, `−3 %`. */
export function changeText(ratio: number | null, language?: string): string {
  if (ratio === null || !Number.isFinite(ratio)) {
    return "–";
  }
  return new Intl.NumberFormat(language, { style: "percent", maximumFractionDigits: 1, signDisplay: "exceptZero" }).format(ratio);
}

type SessionLengths = Schemas["SessionLengths"];

/** The session lengths, shortest first, with their share of the sessions. */
export function lengthShares(lengths: SessionLengths | null | undefined): { key: keyof SessionLengths; count: number; share: number }[] {
  if (!lengths) {
    return [];
  }
  const keys: (keyof SessionLengths)[] = ["under_1_min", "from_1_to_5_min", "from_5_to_30_min", "from_30_min_to_2_h", "over_2_h"];
  const total = keys.reduce((sum, key) => sum + lengths[key], 0);
  return keys.map((key) => ({ key, count: lengths[key], share: total > 0 ? lengths[key] / total : 0 }));
}

/** The periods the page offers, in days, and the default one. */
export const PERIODS = [7, 30, 90, 365] as const;
export const DEFAULT_DAYS = 30;

/** The period of the whole page: the last `days` days up to today (station time). */
export interface Period {
  days: number;
  from: string;
  to: string;
}

/** The period of the page's address (`?days=`), ending `today`. */
export function periodOf(search: URLSearchParams, today: string): Period {
  const asked = Number(search.get("days"));
  const days = (PERIODS as readonly number[]).includes(asked) ? asked : DEFAULT_DAYS;
  return { days, from: addDays(today, 1 - days), to: today };
}

/**
 * Average listeners by day of the week (Monday first) and hour, from the
 * hourly totals: each hour's average weighted by its minutes measured;
 * `null` where nothing was measured.
 */
export function weekHours(rows: PeriodTotals[]): (number | null)[][] {
  const sums = Array.from({ length: 7 }, () => Array.from({ length: 24 }, () => ({ listened: 0, minutes: 0 })));
  for (const row of rows) {
    if (row.hour === null || row.hour === undefined || row.minutes === 0) {
      continue;
    }
    const [y, m, d] = row.day.split("-").map(Number);
    const weekday = (new Date(Date.UTC(y ?? 1970, (m ?? 1) - 1, d ?? 1)).getUTCDay() + 6) % 7;
    const cell = sums[weekday]?.[row.hour];
    if (cell) {
      cell.listened += row.average * row.minutes;
      cell.minutes += row.minutes;
    }
  }
  return sums.map((day) => day.map((cell) => (cell.minutes === 0 ? null : cell.listened / cell.minutes)));
}

/** The days of the week, short, Monday first. */
export function weekdayNames(language?: string): string[] {
  const format = new Intl.DateTimeFormat(language, { weekday: "short", timeZone: "UTC" });
  // 2026-10-19 is a Monday.
  return Array.from({ length: 7 }, (_, i) => format.format(new Date(Date.UTC(2026, 9, 19 + i))));
}

/** How strong a heatmap cell is, 0.08 to 1: an hour with listeners never looks empty. */
export function heat(value: number, top: number): number {
  if (top <= 0 || value <= 0) {
    return 0.08;
  }
  return 0.08 + 0.92 * Math.min(1, value / top);
}

/** A country's name in `language`, from its ISO code. */
export function countryName(code: string, language?: string): string {
  try {
    return new Intl.DisplayNames(language ? [language] : undefined, { type: "region" }).of(code) ?? code;
  } catch {
    return code;
  }
}

/** Whether the GeoIP database is DB-IP's, which asks to be credited. */
export function isDbIp(database: string): boolean {
  return /db-?ip/i.test(database);
}
