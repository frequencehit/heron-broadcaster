// SPDX-License-Identifier: AGPL-3.0-or-later

// The tops (docs/listener-interaction.md#tops): their period, score and
// show as the pages write them, the score's two parts, movements and the
// show's programme.

import type { Schemas } from "../api";
import { locale, t } from "../i18n";

export type Chart = Schemas["Chart"];
export type ChartEntry = Schemas["ChartEntry"];
export type NotRanked = Schemas["NotRanked"];
export type ShowPlan = Schemas["ShowPlan"];
export type ProgrammeItem = Schemas["Item"];
export type TopSettings = Schemas["TopSettings"];
export type TopSlot = Schemas["TopSlot"];
export type TopView = Schemas["TopView"];
export type GivesWay = Schemas["GivesWay"];

/** The sizes a top can have. */
export const SIZES = [3, 5, 10, 20, 30, 40, 50] as const;

/** The months' ordinals a show can play on. */
export const NTHS = ["first", "second", "third", "fourth", "last"] as const;
export type Nth = (typeof NTHS)[number];

/** A new top's settings, as the API's defaults: the week reset on Sunday at 17:00, likes only. */
export function newTopSettings(name: string): TopSettings {
  return {
    name,
    enabled: true,
    size: 30,
    period: "week",
    reset_weekday: 7,
    reset_day: 1,
    reset_time: "17:00",
    plays_percent: 0,
    min_votes: 10,
    category_ids: [],
    jingle_category_id: null,
    new_release_category_id: null,
    hide_unplayed: true,
    slot: null,
    website: { title: "", color: "#c62828", small_widget: true, closed: "message", closed_message: "", page_url: null },
    notifications: { webhook_ids: [], lead_minutes: 60, language: "en", events: {} },
  };
}

/** What the movement column shows: an arrow and the places, `=`, or a word. */
export function movement(entry: Pick<ChartEntry, "movement" | "rank" | "previous_rank">): {
  kind: ChartEntry["movement"];
  places: number | null;
} {
  const previous = entry.previous_rank ?? null;
  const places = previous === null ? null : Math.abs(previous - entry.rank);
  return { kind: entry.movement, places: entry.movement === "up" || entry.movement === "down" ? places : null };
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

/**
 * The score's two parts, in points out of 100: the plays' (`p` × the share
 * of the most played) and the likes' ((1 − `p`) × the share of likes).
 */
export function scoreParts(
  entry: { plays_share?: number | null; share: number; score: number },
  playsPercent: number,
): { plays: number; likes: number; score: number } {
  const p = playsPercent / 100;
  const plays = p * (entry.plays_share ?? 0) * 100;
  const likes = (1 - p) * entry.share * 100;
  return { plays, likes, score: entry.score * 100 };
}

/** `Sunday`, in the current language. */
export function weekdayName(day: number): string {
  // 2024-01-01 was a Monday.
  return new Intl.DateTimeFormat(locale(), { weekday: "long", timeZone: "UTC" }).format(Date.UTC(2024, 0, day));
}

/** The day of the week of a date (`2026-11-08`), 1 (Monday) to 7. */
function weekdayOf(date: Date): number {
  return ((date.getUTCDay() + 6) % 7) + 1;
}

/** Whether a slot plays on a date (UTC midnight of that day). */
export function playsOn(slot: TopSlot, date: Date): boolean {
  switch (slot.every) {
    case "day":
      return true;
    case "week":
      return weekdayOf(date) === slot.weekday;
    case "month": {
      if (weekdayOf(date) !== slot.weekday) {
        return false;
      }
      const day = date.getUTCDate();
      if (slot.nth === "last") {
        const next = new Date(date.getTime() + 7 * 86_400_000);
        return next.getUTCMonth() !== date.getUTCMonth();
      }
      const nth = NTHS.indexOf(slot.nth ?? "first") + 1;
      return Math.ceil(day / 7) === nth;
    }
  }
}

/** The next `count` days a slot plays, from `from` (`2026-11-08`) on. */
export function nextDates(slot: TopSlot, from: string, count: number): string[] {
  const dates: string[] = [];
  let at = Date.parse(`${from}T00:00:00Z`);
  for (let day = 0; day < 400 && dates.length < count; day++, at += 86_400_000) {
    const date = new Date(at);
    if (playsOn(slot, date)) {
      dates.push(date.toISOString().slice(0, 10));
    }
  }
  return dates;
}

function nthLabel(nth: Nth | null | undefined): string {
  switch (nth) {
    case "second":
      return t("tops.nth.second");
    case "third":
      return t("tops.nth.third");
    case "fourth":
      return t("tops.nth.fourth");
    case "last":
      return t("tops.nth.last");
    default:
      return t("tops.nth.first");
  }
}

/** The option of the show's recurrence: `The 1st`, `The last`. */
export function nthOption(nth: Nth): string {
  switch (nth) {
    case "second":
      return t("tops.option.second");
    case "third":
      return t("tops.option.third");
    case "fourth":
      return t("tops.option.fourth");
    case "last":
      return t("tops.option.last");
    default:
      return t("tops.option.first");
  }
}

/** When a show plays without its hours: `Every Sunday`, `1st Sunday of the month`. */
export function recurrenceLabel(slot: TopSlot): string {
  const day = weekdayName(slot.weekday ?? 7);
  switch (slot.every) {
    case "day":
      return t("tops.every.dayLabel");
    case "week":
      return t("tops.every.weekLabel", { day });
    case "month":
      return t("tops.every.monthLabel", { nth: nthLabel(slot.nth), day });
  }
}

/** `1st Sunday of the month, 18:00–20:00`; not on air without a slot. */
export function slotLabel(slot: TopSlot | null | undefined): string {
  if (!slot) {
    return t("tops.noShow");
  }
  return t("tops.slotLabel", { when: recurrenceLabel(slot), start: slot.start, end: slot.end });
}

/** `The week, reset on Sunday at 17:00`. */
export function periodLabel(settings: Pick<TopSettings, "period" | "reset_weekday" | "reset_day" | "reset_time">): string {
  switch (settings.period) {
    case "rolling_7_days":
      return t("tops.period.rolling");
    case "month":
      return t("tops.period.month", { day: settings.reset_day ?? 1, time: settings.reset_time ?? "00:00" });
    default:
      return t("tops.period.week", { day: weekdayName(settings.reset_weekday ?? 7), time: settings.reset_time ?? "00:00" });
  }
}

/** `30 % plays · 70 % likes · 10 votes min.` */
export function scoreLabel(settings: Pick<TopSettings, "plays_percent" | "min_votes">): string {
  const plays = settings.plays_percent ?? 0;
  return t("tops.scoreText", { plays, likes: 100 - plays, count: settings.min_votes ?? 0 });
}

/** The warning of a top that gives way to another on some days. */
export function givesWayLabel(other: GivesWay): string {
  return t("tops.givesWay", { when: recurrenceLabel(other.slot), name: other.name });
}

/** Whether `HH:MM` is a time of the day (`24:00` too, for an end). */
export function validTime(text: string, end = false): boolean {
  const found = /^(\d{2}):(\d{2})$/.exec(text);
  if (!found) {
    return false;
  }
  const minutes = Number(found[1]) * 60 + Number(found[2]);
  return Number(found[2]) < 60 && (minutes < 1440 || (end && minutes === 1440));
}

/** Whether a slot can be saved: its hours, and its day when it needs one. */
export function validSlot(slot: TopSlot): boolean {
  if (!validTime(slot.start) || !validTime(slot.end, true) || slot.end <= slot.start) {
    return false;
  }
  return slot.every === "day" || (slot.weekday ?? 0) >= 1;
}
