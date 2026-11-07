// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  addDays,
  barLabel,
  DEFAULT_DAYS,
  change,
  changeText,
  countryName,
  csvUrl,
  dayOf,
  dayStart,
  decimal,
  filterOf,
  filterParams,
  heat,
  hourLabel,
  isDbIp,
  lengthShares,
  listeningHours,
  measureText,
  percent,
  periodLabel,
  periodOf,
  shortDay,
  statisticsInput,
  weekHours,
  weekdayNames,
} from "./listeners";

describe("days in station time", () => {
  it("writes the day of a time in a zone", () => {
    const at = Date.UTC(2026, 9, 25, 22, 30);
    expect(dayOf(at, "UTC")).toBe("2026-10-25");
    expect(dayOf(at, "Europe/Paris")).toBe("2026-10-25");
    expect(dayOf(at, "Asia/Tokyo")).toBe("2026-10-26");
  });

  it("adds days across months", () => {
    expect(addDays("2026-10-31", 1)).toBe("2026-11-01");
    expect(addDays("2026-03-01", -1)).toBe("2026-02-28");
  });

  it("finds midnight, also on days the clocks change", () => {
    expect(dayStart("2026-10-25", "UTC")).toBe(Date.UTC(2026, 9, 25));
    // Summer time until 03:00: midnight is 22:00 UTC the day before.
    expect(dayStart("2026-10-25", "Europe/Paris")).toBe(Date.UTC(2026, 9, 24, 22));
    expect(dayStart("2026-10-26", "Europe/Paris")).toBe(Date.UTC(2026, 9, 25, 23));
    expect(dayStart("2026-10-26", "America/New_York")).toBe(Date.UTC(2026, 9, 26, 4));
  });

  it("writes ratios as percentages", () => {
    expect(percent(0.125, "en")).toBe("12.5%");
    expect(percent(null, "en")).toBe("–");
  });
});

describe("statistics text", () => {
  const row = {
    day: "2026-10-19",
    days: 7,
    minutes: 10_080,
    peak: 1_250,
    average: 412.345,
    listening_hours: 69_274.1,
    distinct_listeners: 40.25,
  };

  it("writes numbers and listening hours", () => {
    expect(decimal(null)).toBe("–");
    expect(decimal(2.345)).toBe("2.3");
    expect(decimal(2.345, 0)).toBe("2");
    expect(listeningHours(0.2667)).toBe("0.27");
    expect(listeningHours(12.34)).toBe("12.3");
    expect(listeningHours(undefined)).toBe("–");
  });

  it("names periods", () => {
    expect(shortDay("2026-10-19")).toBe("Oct 19");
    expect(periodLabel("2026-10-19", "day")).toBe("Mon, Oct 19");
    expect(periodLabel("2026-10-19", "week")).toBe("Week of Oct 19");
    expect(periodLabel("2026-10-01", "month")).toBe("October 2026");
    expect(barLabel("2026-10-01", "month")).toBe("Oct");
    expect(barLabel("2026-10-19", "week")).toBe("Oct 19");
    expect(hourLabel(8)).toBe("08h");
  });

  it("writes each measure", () => {
    expect(measureText(row, "peak")).toBe("1,250");
    expect(measureText(row, "average")).toBe("412.3");
    expect(measureText(row, "distinct_listeners")).toBe("40.3");
    expect(measureText(row, "listening_hours")).toBe("69,274.1");
  });

  it("links the CSV downloads", () => {
    expect(csvUrl("days", { from: "2026-10-01", to: "2026-10-26", output_id: undefined, group: "week" })).toBe(
      "/api/v1/listeners/days?format=csv&from=2026-10-01&to=2026-10-26&group=week",
    );
  });
});

describe("output statistics settings", () => {
  it("keeps, replaces or removes the admin password", () => {
    expect(statisticsInput(" ", "", false)).toEqual({ admin_username: null });
    expect(statisticsInput("ops", "secret", false)).toEqual({ admin_username: "ops", admin_password: "secret" });
    expect(statisticsInput("", "ignored", true)).toEqual({ admin_username: null, admin_password: null });
  });
});

describe("the page's filter and the audience figures", () => {
  it("reads the output and server from the address", () => {
    expect(filterOf(new URLSearchParams("output=3&server=7"))).toEqual({ output: 3, server: 7 });
    expect(filterOf(new URLSearchParams("server=x&output=0"))).toEqual({ output: undefined, server: undefined });
    expect(filterParams({ output: 3, server: undefined })).toEqual({ output_id: 3, server_id: undefined });
  });

  it("writes the change from the previous period", () => {
    expect(change(120, 100)).toBeCloseTo(0.2);
    expect(change(80, 100)).toBeCloseTo(-0.2);
    expect(change(5, 0)).toBeNull();
    expect(change(null, 3)).toBeNull();
    expect(changeText(0.2, "en")).toBe("+20%");
    expect(changeText(-0.125, "en")).toBe("-12.5%");
    expect(changeText(0, "en")).toBe("0%");
    expect(changeText(null, "en")).toBe("–");
  });

  it("gives each session length its share", () => {
    const shares = lengthShares({ under_1_min: 1, from_1_to_5_min: 1, from_5_to_30_min: 2, from_30_min_to_2_h: 0, over_2_h: 0 });
    expect(shares.map((s) => s.share)).toEqual([0.25, 0.25, 0.5, 0, 0]);
    expect(lengthShares(null)).toEqual([]);
    expect(lengthShares({ under_1_min: 0, from_1_to_5_min: 0, from_5_to_30_min: 0, from_30_min_to_2_h: 0, over_2_h: 0 })[0]?.share).toBe(0);
  });
});

describe("the report's period, heatmap and shares", () => {
  it("reads the period from the address", () => {
    expect(periodOf(new URLSearchParams("days=7"), "2026-11-07")).toEqual({ days: 7, from: "2026-11-01", to: "2026-11-07" });
    expect(periodOf(new URLSearchParams("days=12"), "2026-11-07").days).toBe(DEFAULT_DAYS);
    expect(periodOf(new URLSearchParams(""), "2026-11-07").from).toBe("2026-10-09");
  });

  it("averages the hours by day of the week", () => {
    const row = (day: string, hour: number | null, minutes: number, average: number) =>
      ({ day, hour, minutes, average, days: 1, peak: 0, listening_hours: 0 }) as never;
    // Monday 19 and 26 October at 8, and a Sunday.
    const cells = weekHours([
      row("2026-10-19", 8, 60, 2),
      row("2026-10-26", 8, 30, 8),
      row("2026-10-25", 23, 60, 1),
      row("2026-10-25", null, 60, 9),
      row("2026-10-24", 3, 0, 5),
    ]);
    expect(cells).toHaveLength(7);
    expect(cells[0]?.[8]).toBeCloseTo(4);
    expect(cells[6]?.[23]).toBe(1);
    expect(cells[5]?.[3]).toBeNull();
    expect(cells[1]?.[8]).toBeNull();
  });

  it("names the days, the countries and the database", () => {
    expect(weekdayNames("en")).toEqual(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);
    expect(countryName("FR", "en")).toBe("France");
    expect(countryName("FR", "fr")).toBe("France");
    expect(countryName("DE", "de")).toBe("Deutschland");
    expect(isDbIp("DBIP-Country-Lite")).toBe(true);
    expect(isDbIp("GeoLite2-Country")).toBe(false);
    expect(heat(0, 10)).toBeCloseTo(0.08);
    expect(heat(10, 10)).toBe(1);
    expect(heat(5, 0)).toBeCloseTo(0.08);
  });
});
