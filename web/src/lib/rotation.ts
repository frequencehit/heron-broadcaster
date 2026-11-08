// SPDX-License-Identifier: AGPL-3.0-or-later

// Words for rotation: days, hours, rules, methods, separation windows and
// draw depths.

import type { ClockSlot, Method, Rule, Schemas, SeparationOverrides, Target, Windows } from "../api";
import { type PlainKey, locale, number, t } from "../i18n";
import { clockTime } from "./format";

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
  "rotation.step.11",
  "rotation.step.12",
];

/** What a relaxation step relaxes: `artist separation halved`. */
export function stepLabel(step: number): string {
  return t(STEPS[Math.min(step, STEPS.length - 1)] ?? "rotation.step.12");
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

/** The windows in minutes (daypart separation aside). */
export type MinuteWindow = Exclude<keyof Windows, "dayparts" | "daypart_days">;

const WINDOW_NAMES: [MinuteWindow, PlainKey][] = [
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

/** The windows in minutes overridden: `track 1 h 30, artist off`; empty when none. */
export function describeOverrides(overrides: SeparationOverrides | undefined): string {
  if (!overrides) {
    return "";
  }
  const described = WINDOW_NAMES.filter(([key]) => overrides[key] !== undefined && overrides[key] !== null).map(
    ([key, name]) => t("rotation.window", { name: t(name), length: minutes(overrides[key] ?? 0) }),
  );
  const draw = describeDraw(overrides);
  if (draw) {
    described.push(draw);
  }
  return described.join(", ");
}

/** The draw depth set: `draw 20–50 %`, `draw …–80 %`; empty when none is. */
export function describeDraw(overrides: SeparationOverrides | undefined): string {
  const start = overrides?.draw_start_pct;
  const max = overrides?.draw_max_pct;
  if (start == null && max == null) {
    return "";
  }
  return t("draw.value", { start: start == null ? "…" : number(start), max: max == null ? "…" : number(max) });
}

/** Daypart separation: `dayparts 2`, `dayparts off`. */
function dayparts(value: number): string {
  return t("rotation.window", {
    name: t("rotation.window.dayparts"),
    length: value === 0 ? t("rotation.off") : number(value),
  });
}

/** The same daypart on the following days: `same daypart 2 d`. */
function daypartDays(value: number): string {
  return t("rotation.window", {
    name: t("rotation.window.daypartDays"),
    length: t("format.days", { count: value }),
  });
}

/**
 * Windows as applied: `album 1 h · artist 2 h · title 4 h · track 4 h`, and
 * the dayparts and same daypart on the following days when on.
 */
export function describeWindows(windows: Windows): string {
  const described = WINDOW_NAMES.map(([key, name]) =>
    t("rotation.window", { name: t(name), length: minutes(windows[key]) }),
  );
  if (windows.dayparts) {
    described.push(dayparts(windows.dayparts));
  }
  if (windows.daypart_days) {
    described.push(daypartDays(windows.daypart_days));
  }
  return described.join(" · ");
}

/**
 * Whether the slot at `index` cannot be linked to the next one: a clock of
 * one slot, or a slot next to a linked slot (no chains; the first slot
 * follows the last one). A slot already linked can always be unlinked.
 */
export function linkBlocked(slots: readonly ClockSlot[], index: number): boolean {
  const count = slots.length;
  if (slots[index]?.linked_to_next) {
    return false;
  }
  if (count < 2) {
    return true;
  }
  const previous = slots[(index - 1 + count) % count];
  const next = slots[(index + 1) % count];
  return !!previous?.linked_to_next || !!next?.linked_to_next;
}

/** `:00:00`, `:30:15`: minutes and seconds into the hour. */
export function exactTimeLabel(atS: number): string {
  const minutes = Math.floor(atS / 60);
  const seconds = atS % 60;
  return `:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

/** A slot's exact time: `:00:00 · hard`, `:30:00 · soft, −10 to +30 s`. */
export function describeExact(exact: Schemas["ExactTime"]): string {
  const time = exactTimeLabel(exact.at_s);
  if ((exact.mode ?? "hard") === "hard") {
    return t("exact.hardValue", { time });
  }
  return t("exact.softValue", { time, before: exact.before_s ?? 10, after: exact.after_s ?? 30 });
}

/** How an exact time was met: `on time`, `early by 12 s, outside its window`, `late by 30 s, the track before faded`. */
export function describeMet(log: Schemas["ExactLog"]): string {
  const seconds = Math.round(Math.abs(log.offset_ms) / 1000);
  let met =
    log.met === "on_time"
      ? t("exact.onTime")
      : log.met === "early"
        ? t("exact.early", { seconds })
        : t("exact.late", { seconds });
  if (!log.in_window) {
    met = t("exact.outside", { met });
  }
  return log.faded ? t("exact.faded", { met }) : met;
}

/**
 * How a pick's fill was chosen before an exact time: `best of 10 fills, fit
 * search: ends at 09:59:58, nothing faded, no filling`.
 */
export function describeLookahead(log: Schemas["LookaheadLog"]): string {
  if (log.offset_ms === null || log.offset_ms === undefined) {
    return t("exact.lookaheadNone", { count: log.draws });
  }
  const result = [
    log.faded ? t("exact.lookaheadFaded") : t("exact.lookaheadNothingFaded"),
    log.filling_ms > 0
      ? t("exact.lookaheadFilled", { seconds: Math.round(log.filling_ms / 1000) })
      : t("exact.lookaheadNoFilling"),
    log.relaxed_steps > 0 && t("exact.lookaheadRelaxed", { count: log.relaxed_steps }),
  ]
    .filter(Boolean)
    .join(", ");
  const values = {
    count: log.draws,
    search: log.search ? t("exact.lookaheadSearch") : "",
    time: clockTime(log.exact_at_unix_ms + log.offset_ms),
    result,
  };
  if (log.kept) {
    return log.by_hand ? t("exact.lookaheadByHand", values) : t("exact.lookaheadKept", values);
  }
  return t("exact.lookahead", values);
}

/** A gap filled before an exact time: `medley 7, 1 hook left out`, `a filler`. */
export function describeFill(fill: Schemas["FillLog"]): string {
  if (fill.kind === "filler") {
    return t("exact.filler");
  }
  const id = String(fill.medley_id ?? "?");
  const left = fill.hooks_left_out ?? 0;
  return left > 0 ? t("exact.medleyShortened", { id, count: left }) : t("exact.medley", { id });
}
