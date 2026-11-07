// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  daysLabel,
  describeExact,
  describeFill,
  describeLookahead,
  describeMet,
  describeOverrides,
  describeRule,
  describeWindows,
  exactTimeLabel,
  hourLabel,
  linkBlocked,
  stepLabel,
} from "./rotation";

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
    // The daypart separation has its own column.
    expect(describeOverrides({ track_min: 90, dayparts: 2 })).toBe("track 90 min");
    expect(describeOverrides({ dayparts: 0 })).toBe("");
    expect(describeOverrides({ dayparts: 1, daypart_days: 2 })).toBe("");
    // The draw depth, the start or the maximum alone.
    expect(describeOverrides({ track_min: 90, draw_start_pct: 30, draw_max_pct: 60 })).toBe(
      "track 90 min, draw 30–60 %",
    );
    expect(describeOverrides({ draw_max_pct: 80 })).toBe("draw …–80 %");
  });

  it("lists every applied one", () => {
    expect(describeWindows({ album_min: 60, artist_min: 120, title_min: 0, track_min: 240, dayparts: 0, daypart_days: 0 })).toBe(
      "album 1 h · artist 2 h · title off · track 4 h",
    );
    expect(
      describeWindows({ album_min: 0, artist_min: 120, title_min: 0, track_min: 240, dayparts: 1, daypart_days: 0 }),
    ).toBe("album off · artist 2 h · title off · track 4 h · dayparts 1");
    // The same daypart on the following days, with or without dayparts.
    expect(
      describeWindows({ album_min: 0, artist_min: 0, title_min: 0, track_min: 240, dayparts: 0, daypart_days: 2 }),
    ).toBe("album off · artist off · title off · track 4 h · same daypart 2 d");
  });

  it("names every relaxation step", () => {
    expect(stepLabel(3)).toBe("daypart separation halved");
    expect(stepLabel(4)).toBe("daypart separation dropped");
    expect(stepLabel(5)).toBe("artist separation halved");
    expect(stepLabel(12)).toBe("fallback category, least recently played, no rules");
  });
});

describe("linked slots", () => {
  const slot = (linked_to_next: boolean) => ({ category_id: 1, linked_to_next });

  it("never chain", () => {
    const slots = [slot(true), slot(false), slot(false)];
    expect(linkBlocked(slots, 0)).toBe(false);
    // Next to a linked slot: the second one, and the last one (the first
    // slot follows it).
    expect(linkBlocked(slots, 1)).toBe(true);
    expect(linkBlocked(slots, 2)).toBe(true);
    expect(linkBlocked([slot(false), slot(false), slot(false)], 1)).toBe(false);
    // A clock of one slot cannot link it.
    expect(linkBlocked([slot(false)], 0)).toBe(true);
    // A linked slot can always be unlinked.
    expect(linkBlocked([slot(true)], 0)).toBe(false);
  });
});

describe("exact times", () => {
  it("reads a time in the hour and its mode", () => {
    expect(exactTimeLabel(0)).toBe(":00:00");
    expect(exactTimeLabel(30 * 60 + 15)).toBe(":30:15");
    expect(describeExact({ at_s: 0, mode: "hard" })).toBe(":00:00 · hard");
    expect(describeExact({ at_s: 1800, mode: "soft", before_s: 10, after_s: 30 })).toBe(":30:00 · soft, −10 to +30 s");
  });

  it("says how it was met", () => {
    const log = {
      at_unix_ms: 0,
      mode: "soft" as const,
      before_s: 10,
      after_s: 30,
      met: "late" as const,
      offset_ms: 30_000,
      in_window: true,
      faded: true,
      cut_at_unix_ms: 30_000,
    };
    expect(describeMet(log)).toBe("late by 30 s, the track before faded");
    expect(describeMet({ ...log, met: "early", offset_ms: -60_000, in_window: false, faded: false })).toBe(
      "early by 60 s, outside its window",
    );
    expect(describeMet({ ...log, met: "on_time", offset_ms: 0, faded: false })).toBe("on time");
  });

  it("says how a pick looked ahead", () => {
    const log = {
      exact_at_unix_ms: 3_600_000,
      draws: 8,
      faded: false,
      in_window: true,
      offset_ms: -4_000,
      filling_ms: 0,
      relaxed_steps: 0,
    };
    expect(describeLookahead(log)).toMatch(/^8 endings simulated, the best ends at .*: no filling$/);
    expect(describeLookahead({ ...log, filling_ms: 4_200, relaxed_steps: 2 })).toMatch(
      /: 4 s filled, 2 relaxation steps$/,
    );
    expect(describeLookahead({ ...log, faded: true, in_window: false })).toMatch(
      /: no filling, the track before faded out$/,
    );
    expect(describeLookahead({ ...log, offset_ms: null })).toBe(
      "8 endings simulated, none reaches the exact time",
    );
  });

  it("says what filled the gap", () => {
    const fill = { kind: "medley" as const, slot: 0, exact_at_unix_ms: 0, medley_id: 7, length_ms: 31_000 };
    expect(describeFill({ ...fill, hooks_left_out: 1 })).toBe("medley 7, 1 hook left out");
    expect(describeFill(fill)).toBe("medley 7");
    expect(describeFill({ ...fill, kind: "filler" })).toBe("a filler track");
  });
});
