// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { daysLabel, describeOverrides, describeRule, describeWindows, hourLabel, stepLabel } from "./rotation";

describe("days", () => {
  it("reads as ranges", () => {
    expect(daysLabel([1, 2, 3, 4, 5])).toBe("Mon–Fri");
    expect(daysLabel([6, 7])).toBe("Sat, Sun");
    expect(daysLabel([7, 1, 2, 3, 4, 5, 6])).toBe("every day");
    expect(daysLabel([1, 3, 4, 5, 7])).toBe("Mon, Wed–Fri, Sun");
  });

  it("formats hours", () => {
    expect(hourLabel(6)).toBe("06:00");
    expect(hourLabel(24)).toBe("00:00");
  });
});

describe("rules", () => {
  it("describes dayparting", () => {
    expect(
      describeRule({ kind: "daypart", mode: "ban", days: [1, 2, 3, 4, 5], start_hour: 6, end_hour: 22 }),
    ).toBe("Banned Mon–Fri, 06:00–22:00");
    expect(
      describeRule({ kind: "daypart", mode: "allow", days: [1, 2, 3, 4, 5, 6, 7], start_hour: 0, end_hour: 24 }),
    ).toBe("Only every day, all day");
  });

  it("describes sequences with the category's name", () => {
    expect(describeRule({ kind: "not_after", category_id: 3 }, (id) => (id === 3 ? "Ballads" : undefined))).toBe(
      "Never right after Ballads",
    );
    expect(describeRule({ kind: "not_after", category_id: 4 })).toBe("Never right after category 4");
  });

  it("describes date ranges", () => {
    const rule = describeRule({
      kind: "date_range",
      mode: "allow",
      start: "2000-12-01",
      end: "2000-01-06",
      every_year: true,
    });
    expect(rule).toMatch(/^Only from .+ to .+, every year$/);
    expect(rule).not.toContain("2000");
  });
});

describe("windows", () => {
  it("lists the overridden ones", () => {
    expect(describeOverrides({ track_min: 90, artist_min: 0 })).toBe("artist off, track 90 min");
    expect(describeOverrides({ title_min: 120 })).toBe("title 2 h");
    expect(describeOverrides({})).toBe("");
    expect(describeOverrides({ track_min: 90, dayparts: 2 })).toBe("track 90 min, dayparts 2");
    expect(describeOverrides({ dayparts: 0 })).toBe("dayparts off");
  });

  it("lists every applied one", () => {
    expect(describeWindows({ album_min: 60, artist_min: 120, title_min: 0, track_min: 240, dayparts: 0 })).toBe(
      "album 1 h · artist 2 h · title off · track 4 h",
    );
    expect(describeWindows({ album_min: 0, artist_min: 120, title_min: 0, track_min: 240, dayparts: 1 })).toBe(
      "album off · artist 2 h · title off · track 4 h · dayparts 1",
    );
  });

  it("names every relaxation step", () => {
    expect(stepLabel(3)).toBe("daypart separation halved");
    expect(stepLabel(4)).toBe("daypart separation dropped");
    expect(stepLabel(5)).toBe("artist separation halved");
    expect(stepLabel(12)).toBe("fallback category, least recently played, no rules");
  });
});
