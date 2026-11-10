// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  countdown,
  givesWayLabel,
  leftOutLabel,
  movement,
  newTopSettings,
  nextDates,
  periodLabel,
  playsOn,
  scoreLabel,
  scoreParts,
  slotLabel,
  validSlot,
  validTime,
} from "./tops";

describe("tops", () => {
  it("gives the places a track moved", () => {
    expect(movement({ movement: "up", rank: 2, previous_rank: 5 })).toEqual({ kind: "up", places: 3 });
    expect(movement({ movement: "down", rank: 7, previous_rank: 5 })).toEqual({ kind: "down", places: 2 });
    expect(movement({ movement: "same", rank: 1, previous_rank: 1 })).toEqual({ kind: "same", places: null });
    expect(movement({ movement: "new", rank: 6, previous_rank: null })).toEqual({ kind: "new", places: null });
  });

  it("writes the ranks left out as a range", () => {
    expect(leftOutLabel([])).toBe("");
    expect(leftOutLabel([30])).toBe("30");
    expect(leftOutLabel([28, 29, 30])).toBe("30–28");
  });

  it("counts the tracks the show plays down from", () => {
    const item = (kind: "track" | "jingle" | "new_release", rank: number | null, skipped: string | null = null) => ({
      kind,
      rank,
      skipped,
    });
    expect(
      countdown({
        items: [item("jingle", 3), item("track", 3), item("track", 2, "restriction"), item("track", 1), item("new_release", null)],
      } as never),
    ).toEqual({ tracks: 2, from: 3 });
    expect(countdown({ items: [] })).toEqual({ tracks: 0, from: null });
  });

  it("splits the score into the plays' part and the likes'", () => {
    // 50 %: the most played (100 %) liked at 90 %.
    expect(scoreParts({ plays_share: 1, share: 0.9, score: 0.95 }, 50)).toEqual({ plays: 50, likes: 45, score: 95 });
    // Likes only, as the Top 30 ranked them.
    expect(scoreParts({ plays_share: null, share: 0.8, score: 0.8 }, 0)).toEqual({ plays: 0, likes: 80, score: 80 });
    const plays = scoreParts({ plays_share: 0.5, share: 0, score: 0.5 }, 100);
    expect(plays.plays).toBe(50);
    expect(plays.likes).toBe(0);
  });

  it("finds the days a show plays", () => {
    // 6 December 2026 is the first Sunday of the month; 29 November the last.
    const day = (text: string) => new Date(`${text}T00:00:00Z`);
    const first = { every: "month", weekday: 7, nth: "first", start: "18:00", end: "20:00" } as const;
    expect(playsOn(first, day("2026-12-06"))).toBe(true);
    expect(playsOn(first, day("2026-12-13"))).toBe(false);
    const last = { ...first, nth: "last" } as const;
    expect(playsOn(last, day("2026-11-29"))).toBe(true);
    expect(playsOn(last, day("2026-11-22"))).toBe(false);
    expect(nextDates(first, "2026-11-08", 3)).toEqual(["2026-12-06", "2027-01-03", "2027-02-07"]);
    expect(nextDates({ every: "week", weekday: 1, nth: null, start: "09:00", end: "10:00" }, "2026-11-08", 2)).toEqual([
      "2026-11-09",
      "2026-11-16",
    ]);
    expect(nextDates({ every: "day", weekday: null, nth: null, start: "20:00", end: "21:00" }, "2026-11-08", 2)).toEqual([
      "2026-11-08",
      "2026-11-09",
    ]);
  });

  it("writes the period, the score and the show", () => {
    const settings = newTopSettings("Top 30");
    expect(periodLabel(settings)).toBe("The week, reset on Sunday at 17:00");
    expect(periodLabel({ ...settings, period: "month", reset_day: 1, reset_time: "00:00" })).toBe(
      "The month, reset on day 1 at 00:00",
    );
    expect(periodLabel({ ...settings, period: "rolling_7_days" })).toBe("The last 7 days, frozen before each show");
    expect(scoreLabel({ plays_percent: 30, min_votes: 10 })).toBe("30 % plays · 70 % likes · 10 votes min.");
    expect(scoreLabel({ plays_percent: 0, min_votes: 1 })).toBe("0 % plays · 100 % likes · 1 vote min.");
    expect(slotLabel(null)).toBe("Not on air");
    expect(slotLabel({ every: "day", weekday: null, nth: null, start: "20:00", end: "21:00" })).toBe("Every day, 20:00–21:00");
    expect(slotLabel({ every: "week", weekdays: [1, 2, 3, 4, 5, 6, 7], nth: null, start: "20:00", end: "21:00" })).toBe(
      "Every day, 20:00–21:00",
    );
    expect(slotLabel({ every: "week", weekdays: [5, 1, 3], nth: null, start: "20:00", end: "21:00" })).toBe(
      "Every Monday, Wednesday, and Friday, 20:00–21:00",
    );
    expect(slotLabel({ every: "month", weekdays: [6, 7], nth: "first", start: "18:00", end: "20:00" })).toBe(
      "1st Saturday and Sunday of the month, 18:00–20:00",
    );
    expect(slotLabel({ every: "week", weekday: 7, nth: null, start: "18:00", end: "20:00" })).toBe(
      "Every Sunday, 18:00–20:00",
    );
    expect(slotLabel({ every: "month", weekday: 7, nth: "first", start: "18:00", end: "20:00" })).toBe(
      "1st Sunday of the month, 18:00–20:00",
    );
    expect(
      givesWayLabel({
        top_id: 2,
        name: "Top 30 of the month",
        slot: { every: "month", weekday: 7, nth: "first", start: "18:00", end: "20:00" },
      }),
    ).toBe("1st Sunday of the month, “Top 30 of the month” takes the same slot: this one gives way that day.");
  });

  it("checks times and slots before saving", () => {
    expect(validTime("17:00")).toBe(true);
    expect(validTime("24:00")).toBe(false);
    expect(validTime("24:00", true)).toBe(true);
    expect(validTime("7:00")).toBe(false);
    expect(validTime("12:60")).toBe(false);
    expect(validSlot({ every: "day", weekday: null, nth: null, start: "20:00", end: "24:00" })).toBe(true);
    expect(validSlot({ every: "day", weekday: null, nth: null, start: "20:00", end: "19:00" })).toBe(false);
    expect(validSlot({ every: "week", weekday: null, nth: null, start: "18:00", end: "20:00" })).toBe(false);
    expect(validSlot({ every: "week", weekdays: [], nth: null, start: "18:00", end: "20:00" })).toBe(false);
    expect(validSlot({ every: "week", weekdays: [1, 3, 5], nth: null, start: "18:00", end: "20:00" })).toBe(true);
  });
});
