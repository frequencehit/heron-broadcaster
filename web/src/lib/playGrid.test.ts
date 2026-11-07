// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { type GridDay, dateLabel, historyFilter, historyHref, hourCells, most, shade, weeks } from "./playGrid";

/** The 30 days ending Thursday 5 November 2026, with `plays` plays on some. */
function days(plays: Record<string, number> = {}): GridDay[] {
  const out: GridDay[] = [];
  for (let index = 0; index < 30; index++) {
    const ms = Date.UTC(2026, 9, 7) + index * 86_400_000;
    const date = new Date(ms).toISOString().slice(0, 10);
    out.push({
      date,
      weekday: ((new Date(ms).getUTCDay() + 6) % 7) + 1,
      start_unix_ms: ms,
      end_unix_ms: ms + 86_400_000,
      plays: Array.from({ length: plays[date] ?? 0 }, (_, n) => ({
        started_at_unix_ms: ms + n * 60_000,
        date,
        time: `08:0${n}`,
        station_id: 1,
        source: "rotation" as const,
        likes: 0,
        dislikes: 0,
      })),
    });
  }
  return out;
}

describe("weeks", () => {
  it("puts the 30 days in weeks from Monday, the days around them greyed out", () => {
    const rows = weeks(days({ "2026-10-07": 1, "2026-10-12": 2, "2026-10-18": 1, "2026-11-05": 3 }));
    expect(rows.map((week) => week.monday)).toEqual([
      "2026-10-05",
      "2026-10-12",
      "2026-10-19",
      "2026-10-26",
      "2026-11-02",
    ]);
    // Wednesday 7 October is the first of the 30 days.
    expect(rows[0]?.cells.map((cell) => cell.day !== null)).toEqual([false, false, true, true, true, true, true]);
    // Thursday 5 November is today: the days after are out.
    expect(rows[4]?.cells.map((cell) => cell.day !== null)).toEqual([true, true, true, true, false, false, false]);
    expect(rows[4]?.cells[6]?.date).toBe("2026-11-08");
    expect(rows.map((week) => week.total)).toEqual([1, 3, 0, 0, 3]);
  });

  it("is empty without days", () => {
    expect(weeks([])).toEqual([]);
  });
});

describe("shade", () => {
  it("steps by one play up to 4, then by quarters of the most", () => {
    expect([0, 1, 2, 3, 4, 7].map((count) => shade(count, 3))).toEqual([0, 1, 2, 3, 4, 4]);
    expect([1, 5, 10, 11, 20].map((count) => shade(count, 20))).toEqual([1, 1, 2, 3, 4]);
  });
});

describe("hours", () => {
  it("finds a cell by weekday and hour, and the most plays", () => {
    const play = {
      started_at_unix_ms: 0,
      date: "2026-11-02",
      time: "08:12",
      station_id: 1,
      source: "rotation" as const,
      likes: 0,
      dislikes: 0,
    };
    const cells = hourCells([
      { weekday: 1, hour: 8, plays: [play, play] },
      { weekday: 7, hour: 2, plays: [play] },
    ]);
    expect(cells.get("1-8")?.plays).toHaveLength(2);
    expect(cells.get("7-2")?.hour).toBe(2);
    expect(cells.get("1-9")).toBeUndefined();
    expect(most([...cells.values()])).toBe(2);
    expect(most([])).toBe(0);
  });
});

describe("dates", () => {
  it("writes a day in the current language, whatever the browser's zone", () => {
    expect(dateLabel("2026-10-12")).toBe("Oct 12");
    expect(dateLabel("2026-10-12", true)).toBe("Mon, Oct 12");
  });
});

describe("history", () => {
  it("links a day of a track to the history, and reads it back", () => {
    const day = days()[0] as GridDay;
    const href = historyHref(42, day);
    expect(href).toBe(`/history?track=42&after=${day.start_unix_ms}&before=${day.end_unix_ms}`);
    const search = new URL(href, "http://heron").searchParams;
    expect(historyFilter(search)).toEqual({ trackId: 42, after: day.start_unix_ms, before: day.end_unix_ms });
    expect(historyFilter(new URLSearchParams("track=42"))).toEqual({ trackId: 42, after: undefined, before: undefined });
    expect(historyFilter(new URLSearchParams(""))).toBeNull();
    expect(historyFilter(new URLSearchParams("track=abc"))).toBeNull();
  });
});
