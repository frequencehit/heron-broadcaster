// SPDX-License-Identifier: AGPL-3.0-or-later

// Words for rotation: days, hours, rules, methods and separation windows.

import type { Method, Rule, SeparationOverrides, Target, Windows } from "../api";
import { type PlainKey, locale, number, t } from "../i18n";

/** Monday (`1`) to Sunday (`7`). */
export const DAY_NUMBERS = [1, 2, 3, 4, 5, 6, 7] as const;

/** `1` (Monday) to `7` (Sunday), short, in the current language: `Mon`, `lun.`. */
export function dayName(day: number): string {
  if (day < 1 || day > 7) {
    return String(day);
  }
  // 2024-01-01 was a Monday.
  return new Intl.DateTimeFormat(locale(), { weekday: "short", timeZone: "UTC" }).format(
    Date.UTC(2024, 0, day),
  );
}

/** `06:00`; 24 is midnight at the end of the day. */
export function hourLabel(hour: number): string {
  return `${String(hour % 24).padStart(2, "0")}:00`;
}

/** `Mon–Fri`, `Sat, Sun`, `every day`. */
export function daysLabel(days: number[]): string {
  const sorted = [...new Set(days)].filter((d) => d >= 1 && d <= 7).sort((a, b) => a - b);
  if (sorted.length === 7) {
    return t("rotation.everyDay");
  }
  const runs: number[][] = [];
  for (const day of sorted) {
    const run = runs.at(-1);
    if (run?.at(-1) === day - 1) {
      run.push(day);
    } else {
      runs.push([day]);
    }
  }
  return runs
    .map((run) => {
      const first = dayName(run[0] ?? 1);
      const last = dayName(run.at(-1) ?? 1);
      if (run.length >= 3) {
        return `${first}–${last}`;
      }
      return run.map(dayName).join(", ");
    })
    .join(", ");
}

const METHODS: Record<Method, PlainKey> = {
  least_recently_played: "rotation.method.leastRecentlyPlayed",
  random: "rotation.method.random",
  weighted: "rotation.method.weighted",
};

export function methodLabel(method: Method): string {
  return t(METHODS[method]);
}

// The relaxation steps, as heron-rotation describes them (`describe_step`).
const STEPS: PlainKey[] = [
  "rotation.step.0",
  "rotation.step.1",
  "rotation.step.2",
  "rotation.step.3",
  "rotation.step.4",
  "rotation.step.5",
  "rotation.step.6",
  "rotation.step.7",
  "rotation.step.8",
  "rotation.step.9",
  "rotation.step.10",
];

/** What a relaxation step relaxes: `artist separation halved`. */
export function stepLabel(step: number): string {
  return t(STEPS[Math.min(step, STEPS.length - 1)] ?? "rotation.step.10");
}

export const METHODS_LIST = Object.keys(METHODS) as Method[];

function dateLabel(date: string, everyYear: boolean): string {
  const parsed = new Date(`${date}T00:00:00Z`);
  if (Number.isNaN(parsed.getTime())) {
    return date;
  }
  const options: Intl.DateTimeFormatOptions = everyYear
    ? { day: "numeric", month: "long", timeZone: "UTC" }
    : { dateStyle: "medium", timeZone: "UTC" };
  return new Intl.DateTimeFormat(locale(), options).format(parsed);
}

/** A rule for people: `Banned Mon–Fri 06:00–10:00`. */
export function describeRule(rule: Rule, categoryName?: (id: number) => string | undefined): string {
  switch (rule.kind) {
    case "daypart": {
      const hours =
        rule.start_hour === 0 && rule.end_hour === 24
          ? t("rotation.allDay")
          : `${hourLabel(rule.start_hour)}–${hourLabel(rule.end_hour)}`;
      const days = daysLabel(rule.days);
      return rule.mode === "ban"
        ? t("rotation.rule.daypartBan", { days, hours })
        : t("rotation.rule.daypartOnly", { days, hours });
    }
    case "date_range": {
      const range = { start: dateLabel(rule.start, rule.every_year), end: dateLabel(rule.end, rule.every_year) };
      if (rule.mode === "ban") {
        return t(rule.every_year ? "rotation.rule.datesBanYearly" : "rotation.rule.datesBan", range);
      }
      return t(rule.every_year ? "rotation.rule.datesOnlyYearly" : "rotation.rule.datesOnly", range);
    }
    case "not_after": {
      const name = categoryName?.(rule.category_id) ?? t("rotation.categoryNumber", { id: String(rule.category_id) });
      return t("rotation.rule.notAfter", { category: name });
    }
  }
}

export function targetKind(target: Target): string {
  const kinds: Record<Target["kind"], PlainKey> = {
    track: "rotation.target.track",
    artist: "rotation.target.artist",
    category: "rotation.target.category",
  };
  return t(kinds[target.kind]);
}

const WINDOW_NAMES: [keyof Windows, PlainKey][] = [
  ["album_min", "rotation.window.album"],
  ["artist_min", "rotation.window.artist"],
  ["title_min", "rotation.window.title"],
  ["track_min", "rotation.window.track"],
];

export const WINDOW_FIELDS = WINDOW_NAMES;

function minutes(value: number): string {
  if (value === 0) {
    return t("rotation.off");
  }
  if (value % 60 === 0) {
    return t("rotation.hours", { hours: number(value / 60) });
  }
  return t("rotation.minutes", { minutes: number(value) });
}

/** The windows overridden: `track 1 h 30, artist off`; empty when none. */
export function describeOverrides(overrides: SeparationOverrides | undefined): string {
  if (!overrides) {
    return "";
  }
  return WINDOW_NAMES.filter(([key]) => overrides[key] !== undefined && overrides[key] !== null)
    .map(([key, name]) => t("rotation.window", { name: t(name), length: minutes(overrides[key] ?? 0) }))
    .join(", ");
}

/** Windows as applied: `album 1 h · artist 2 h · title 4 h · track 4 h`. */
export function describeWindows(windows: Windows): string {
  return WINDOW_NAMES.map(([key, name]) => t("rotation.window", { name: t(name), length: minutes(windows[key]) })).join(" · ");
}
