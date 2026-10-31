// SPDX-License-Identifier: AGPL-3.0-or-later

// Listener statistics: the current counts, and the days of a period in
// station time (see docs/statistics.md).

import { useQuery } from "@tanstack/react-query";
import { type Schemas, api, unwrap } from "../api";
import { locale, number, t } from "../i18n";
import { here, stationUrl } from "./station";

/** Each output's listeners at the last minute; read again every half minute. */
export function useListenersNow() {
  return useQuery({
    queryKey: ["listeners", "now"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/listeners/now", { params: { path: here() } })),
    refetchInterval: 30_000,
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
