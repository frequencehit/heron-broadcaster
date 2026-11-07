// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { clashes, fillMessage, hourLabel, pageUrl, unknownPlaceholders, validRange, weekGrid, widgetLine } from "./shows";

describe("shows", () => {
  it("draws ranges as a week of whole hours", () => {
    const grid = weekGrid([
      { days: [1, 2, 3, 4, 5], start_hour: 17, end_hour: 18 },
      { days: [7], start_hour: 22, end_hour: 24 },
    ]);
    expect(grid[0]?.[17]).toBe(true);
    expect(grid[0]?.[18]).toBe(false);
    expect(grid[5]?.[17]).toBe(false);
    expect(grid[6]?.[23]).toBe(true);
  });

  it("finds the hours another show has", () => {
    const match = { module: "match", hours: [{ days: [6], start_hour: 18, end_hour: 20 }] };
    const found = clashes(
      [
        { days: [1, 2, 3, 4, 5], start_hour: 17, end_hour: 18 },
        { days: [6], start_hour: 17, end_hour: 19 },
      ],
      [match],
    );
    expect(found).toEqual([null, { module: "match", day: 6, start: 18, end: 19 }]);
  });

  it("checks ranges", () => {
    expect(validRange({ days: [1], start_hour: 17, end_hour: 18 })).toBe(true);
    expect(validRange({ days: [], start_hour: 17, end_hour: 18 })).toBe(false);
    expect(validRange({ days: [1], start_hour: 18, end_hour: 18 })).toBe(false);
  });

  it("fills messages and finds unknown placeholders", () => {
    expect(fillMessage("{name} until {end} {other}", { name: "Dedications", end: "18:00" })).toBe(
      "Dedications until 18:00 {other}",
    );
    expect(unknownPlaceholders("{name} {artist} {artist}", ["name"])).toEqual(["artist"]);
  });

  it("writes the lines for the website", () => {
    expect(widgetLine("https://public.radio.example.org/", "main", "dedications", true)).toBe(
      '<script src="https://public.radio.example.org/api/v1/public/stations/main/dedications.js" data-mode="page" async></script>',
    );
    expect(widgetLine("https://g.example", "main", "match", false)).toBe(
      '<script src="https://g.example/api/v1/public/stations/main/match.js" async></script>',
    );
    expect(pageUrl("https://g.example", "main", "match")).toBe("https://g.example/api/v1/public/stations/main/match.html");
    expect(hourLabel(9)).toBe("09:00");
  });
});
