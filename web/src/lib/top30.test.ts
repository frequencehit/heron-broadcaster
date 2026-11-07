// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { countdown, leftOutLabel, movement, weekDays } from "./top30";

describe("top 30", () => {
  it("gives the places a track moved", () => {
    expect(movement({ movement: "up", rank: 2, previous_rank: 5 })).toEqual({ kind: "up", places: 3 });
    expect(movement({ movement: "down", rank: 7, previous_rank: 5 })).toEqual({ kind: "down", places: 2 });
    expect(movement({ movement: "same", rank: 1, previous_rank: 1 })).toEqual({ kind: "same", places: null });
    expect(movement({ movement: "new", rank: 6, previous_rank: null })).toEqual({ kind: "new", places: null });
  });

  it("names a week by its Monday and Sunday", () => {
    const { monday, sunday } = weekDays("2026-11-02");
    expect(new Date(monday).toISOString()).toBe("2026-11-02T00:00:00.000Z");
    expect(new Date(sunday).toISOString()).toBe("2026-11-08T00:00:00.000Z");
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
});
