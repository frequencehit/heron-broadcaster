// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { areaPath, barLayout, barPath, linePath, nearest, niceScale, splitAtGaps, timeTicks, tooltipSide } from "./chart";

const identity = (value: number) => value;

describe("scales", () => {
  it("rounds the top of the axis", () => {
    expect(niceScale(0)).toEqual({ top: 1, ticks: [0, 1] });
    expect(niceScale(3)).toEqual({ top: 3, ticks: [0, 1, 2, 3] });
    expect(niceScale(37)).toEqual({ top: 40, ticks: [0, 10, 20, 30, 40] });
    expect(niceScale(0.27)).toEqual({ top: 1, ticks: [0, 1] });
  });

  it("puts a tick every 3 hours, every 6 on narrow charts, not at the end", () => {
    const day = 24 * 3_600_000;
    expect(timeTicks(0, day, 800)).toHaveLength(8);
    expect(timeTicks(0, day, 400)).toEqual([0, 6, 12, 18].map((h) => h * 3_600_000));
  });
});

describe("lines", () => {
  const minute = 60_000;
  const points = [0, 1, 2, 5, 9, 10].map((m) => ({ at: m * minute, value: m }));

  it("breaks where minutes are missing", () => {
    expect(splitAtGaps(points).map((s) => s.map((p) => p.value))).toEqual([[0, 1, 2], [5], [9, 10]]);
    expect(splitAtGaps([])).toEqual([]);
    // Days or hours: one apart.
    expect(splitAtGaps([{ at: 0, value: 1 }, { at: 1, value: 2 }, { at: 3, value: 3 }], 1).map((s) => s.length)).toEqual([2, 1]);
  });

  it("draws lines and areas", () => {
    const segment = [
      { at: 0, value: 2 },
      { at: 10, value: 4 },
    ];
    expect(linePath(segment, identity, identity)).toBe("M0.0,2.0L10.0,4.0");
    expect(areaPath(segment, identity, identity)).toBe("M0.0,2.0L10.0,4.0L10.0,0L0.0,0Z");
    expect(areaPath([], identity, identity)).toBe("");
  });

  it("finds the nearest minute, not too far", () => {
    expect(nearest(points, 4 * minute)?.value).toBe(5);
    expect(nearest(points, 60 * minute)).toBeNull();
    expect(nearest([], 0)).toBeNull();
  });
});

describe("bars", () => {
  it("lays bars out with gaps and sparse labels", () => {
    const few = barLayout(2, 600);
    expect(few.barWidth).toBe(48);
    expect(few.every).toBe(1);
    const many = barLayout(365, 600);
    expect(many.barWidth).toBe(1);
    expect(many.every).toBeGreaterThan(1);
  });

  it("rounds the data end only", () => {
    expect(barPath(0, 10, 100, 0)).toBe("");
    expect(barPath(0, 10, 100, 50)).toBe("M0,100V54Q0,50 4,50H6Q10,50 10,54V100Z");
    // Lower than the radius: the rounding shrinks.
    expect(barPath(0, 10, 100, 2)).toBe("M0,100V100Q0,98 2,98H8Q10,98 10,100V100Z");
  });

  it("keeps tooltips inside the chart", () => {
    expect(tooltipSide(100, 600)).toEqual({ left: 108 });
    expect(tooltipSide(500, 600)).toEqual({ right: 108 });
  });
});
