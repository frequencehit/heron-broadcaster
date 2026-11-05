// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { cells, effectiveRatio, nextText, plays, ratioFilter, ratioLabel, ratioSummary } from "./ratio";

describe("play ratio", () => {
  it("plays in the first steps of the counter", () => {
    expect([10, 9, 8, 7, 6, 5, 4, 3, 2, 1].filter((counter) => plays(2, counter))).toEqual([10, 9]);
    expect([10, 5, 1].every((counter) => plays(10, counter))).toBe(true);
    expect(plays(0, 10)).toBe(false);
  });

  it("draws a counter from 10 down to 1, the next step marked", () => {
    const drawn = cells(2, 8);
    expect(drawn.map((cell) => cell.step)).toEqual([10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
    expect(drawn.filter((cell) => cell.plays).map((cell) => cell.step)).toEqual([10, 9]);
    expect(drawn.filter((cell) => cell.next).map((cell) => cell.step)).toEqual([8]);
  });

  it("reads as a number, in tenths", () => {
    expect(ratioLabel(2)).toBe("0.2");
    expect(ratioLabel(10)).toBe("1");
    expect(ratioLabel(0)).toBe("0");
  });

  it("says what a ratio does", () => {
    expect(ratioSummary(2)).toBe("Plays 2 times out of 10, then is held back 8 times.");
    expect(ratioSummary(10)).toBe("Plays every time rotation picks it.");
    expect(ratioSummary(0)).toMatch(/disabled/);
  });

  it("says what the next pick does", () => {
    expect(nextText(2, 9)).toBe("the next pick plays");
    expect(nextText(2, 8)).toBe("held back at the next 8 picks, then plays again");
    expect(nextText(2, 1)).toBe("held back at the next pick, then plays again");
    expect(nextText(0, 10)).toBe("it does not play");
  });

  it("is 0 for a disabled track, which keeps its own", () => {
    expect(effectiveRatio({ enabled: true, play_ratio: 3 })).toBe(3);
    expect(effectiveRatio({ enabled: false, play_ratio: 3 })).toBe(0);
  });

  it("filters the track list", () => {
    expect(ratioFilter("below")).toEqual({ ratio_min: 1, ratio_max: 9 });
    expect(ratioFilter("low")).toEqual({ ratio_min: 1, ratio_max: 3 });
    expect(ratioFilter("off")).toEqual({ ratio_max: 0 });
    expect(ratioFilter(null)).toEqual({});
    expect(ratioFilter("toString")).toEqual({});
  });
});
