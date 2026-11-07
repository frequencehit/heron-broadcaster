// SPDX-License-Identifier: AGPL-3.0-or-later

// The shows (docs/listener-interaction.md): their hours as a week, the
// hours the other shows already have, messages with their placeholders,
// and the lines that put their page and small widget on the website.

import type { Schemas } from "../api";
import type { IconName } from "../components/ui";
import type { PlainKey } from "../i18n";

export type ShowKey = "dedications" | "match" | "top30" | "likes";
export type ShowSettings = Schemas["ShowSettings"];
export type HourRange = Schemas["HourRange"];
export type ShowView = Schemas["ShowView"];

/** The shows, in the order of the menu. Each show adds its own tabs to its page. */
export const SHOWS: { key: ShowKey; label: PlainKey; icon: IconName; hours: boolean }[] = [
  { key: "dedications", label: "nav.dedications", icon: "mic", hours: true },
  { key: "match", label: "nav.match", icon: "bolt", hours: true },
  { key: "top30", label: "nav.top30", icon: "chart", hours: true },
  // Open all the time: no hours, no notifications.
  { key: "likes", label: "nav.likes", icon: "like", hours: false },
];

/** Whether a show opens at set hours (Likes are always open). */
export function hasHours(key: string): boolean {
  return SHOWS.find((show) => show.key === key)?.hours ?? true;
}

export function showLabel(key: string): PlainKey {
  return SHOWS.find((show) => show.key === key)?.label ?? "nav.shows";
}

/** The events every show sends, with their label; a show adds its own after them. */
export const COMMON_EVENTS: { name: string; label: PlainKey }[] = [
  { name: "show.upcoming", label: "shows.event.upcoming" },
  { name: "show.started", label: "shows.event.started" },
  { name: "show.ended", label: "shows.event.ended" },
];

/** The placeholders of every show's messages. */
export const PLACEHOLDERS = ["name", "link", "end", "next"];

/** A show's own events, after the common ones. */
export function ownEvents(key: string): { name: string; label: PlainKey }[] {
  switch (key) {
    case "top30":
      return [
        { name: "top.published", label: "top30.event.published" },
        { name: "top.rank", label: "top30.event.rank" },
      ];
    case "dedications":
      return [
        { name: "dedication.played", label: "dedications.event.played" },
        { name: "dedication.accepted", label: "dedications.event.accepted" },
      ];
    case "match":
      return [
        { name: "match.started", label: "match.event.started" },
        { name: "match.closed", label: "match.event.closed" },
      ];
    default:
      return [];
  }
}

/** A show's own placeholders. */
export function ownPlaceholders(key: string): string[] {
  switch (key) {
    case "top30":
      return ["artist", "title", "top3", "rank"];
    case "dedications":
      return ["artist", "title", "from", "to", "message", "time"];
    case "match":
      return ["title1", "artist1", "title2", "artist2", "close", "winner", "loser", "percent", "votes"];
    default:
      return [];
  }
}

/** Example values of a show's own placeholders, for the preview. */
export function ownExample(key: string): Record<string, string> {
  switch (key) {
    case "top30":
      return {
        artist: "Mira Solano",
        title: "Sunday Market",
        top3: "1. Mira Solano – Sunday Market, 2. Cassia Bloom – Neon Orchard, 3. Saltwater Choir – Lighthouse Keeper",
        rank: "12",
      };
    case "dedications":
      return {
        artist: "Mira Solano",
        title: "Sunday Market",
        from: "Nadia",
        to: "Papi Jean",
        message: "Happy 80th!",
        time: "17:36",
      };
    case "match":
      return {
        title1: "Neon Orchard",
        artist1: "Cassia Bloom",
        title2: "Running Lights",
        artist2: "Echo Parade",
        close: "18:05",
        winner: "Neon Orchard",
        loser: "Running Lights",
        percent: "58 %",
        votes: "79",
      };
    default:
      return {};
  }
}

/** 7 days (0 is Monday) of 24 hours: whether each hour is in the ranges. */
export function weekGrid(ranges: readonly HourRange[]): boolean[][] {
  const grid = Array.from({ length: 7 }, () => Array<boolean>(24).fill(false));
  for (const range of ranges) {
    for (const day of range.days) {
      const row = grid[day - 1];
      if (!row) continue;
      for (let hour = range.start_hour; hour < Math.min(range.end_hour, 24); hour++) {
        row[hour] = true;
      }
    }
  }
  return grid;
}

/** Where a range takes hours another show has. */
export interface Clash {
  module: string;
  day: number;
  start: number;
  end: number;
}

/** The first clash of each range with the other shows' hours, or `null`. */
export function clashes(ranges: readonly HourRange[], others: readonly { module: string; hours: HourRange[] }[]): (Clash | null)[] {
  const theirs = others.map((other) => ({ module: other.module, grid: weekGrid(other.hours) }));
  return ranges.map((range) => {
    for (const day of [...range.days].sort((a, b) => a - b)) {
      for (const other of theirs) {
        let start: number | null = null;
        let end = 0;
        for (let hour = range.start_hour; hour < Math.min(range.end_hour, 24); hour++) {
          if (other.grid[day - 1]?.[hour]) {
            start ??= hour;
            end = hour + 1;
          } else if (start !== null) {
            break;
          }
        }
        if (start !== null) {
          return { module: other.module, day, start, end };
        }
      }
    }
    return null;
  });
}

/** Whether a range is whole: days, and an end after its start. */
export function validRange(range: HourRange): boolean {
  return range.days.length > 0 && range.start_hour >= 0 && range.start_hour < range.end_hour && range.end_hour <= 24;
}

/** `{name}`-style placeholders of a message, filled; unknown ones stay. */
export function fillMessage(template: string, values: Record<string, string>): string {
  return template.replace(/\{([a-z0-9_.]+)\}/g, (all, name: string) => values[name] ?? all);
}

/** The placeholders of a message that are not in `known`. */
export function unknownPlaceholders(template: string, known: readonly string[]): string[] {
  const found = [...template.matchAll(/\{([a-z0-9_.]+)\}/g)].map((m) => m[1] ?? "");
  return [...new Set(found.filter((name) => !known.includes(name)))];
}

/** The script line of a show's page (`page`) or small widget, for the website. */
export function widgetLine(gatewayUrl: string, code: string, module: string, page: boolean): string {
  const base = gatewayUrl.replace(/\/+$/, "");
  const mode = page ? ' data-mode="page"' : "";
  return `<script src="${base}/api/v1/public/stations/${code}/${module}.js"${mode} async></script>`;
}

/** The address of a show's page alone, on the gateway. */
export function pageUrl(gatewayUrl: string, code: string, module: string): string {
  return `${gatewayUrl.replace(/\/+$/, "")}/api/v1/public/stations/${code}/${module}.html`;
}

/** `17:00` for an hour of the day, 0 to 24. */
export function hourLabel(hour: number): string {
  return `${String(hour).padStart(2, "0")}:00`;
}
