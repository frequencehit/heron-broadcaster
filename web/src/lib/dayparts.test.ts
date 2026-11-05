// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { daypartHours, daypartsList, describeSeparation, exampleDays, maxSeparation, partName } from "./dayparts";

describe("dayparts", () => {
  it("divides the day in 4 or 6", () => {
    expect(daypartsList(4)).toBe("A 00:00–05:59 · B 06:00–11:59 · C 12:00–17:59 · D 18:00–23:59");
    expect(daypartHours(6, 5)).toBe("20:00–23:59");
    expect(maxSeparation(4)).toBe(6);
    expect(maxSeparation(6)).toBe(10);
  });

  it("names the dayparts of the next days", () => {
    expect(partName(4, 2)).toBe("C");
    expect(partName(4, 4)).toBe("tomorrow's A");
    expect(partName(6, 13)).toBe("B in 2 days");
  });

  it("draws an example over as many days as it needs", () => {
    // Played in A, 1: B kept out.
    expect(exampleDays(4, 1, 0)).toEqual([
      ["played", "kept", "free", "free"],
      ["free", "free", "free", "free"],
    ]);
    // Played in D, 1: tomorrow's A kept out, across midnight.
    expect(exampleDays(4, 1, 3)[1]).toEqual(["kept", "free", "free", "free"]);
    // Played in A, 6: up to tomorrow's C.
    expect(exampleDays(4, 6, 0)).toEqual([
      ["played", "kept", "kept", "kept"],
      ["kept", "kept", "kept", "free"],
    ]);
    // Played in F, 10 with 6 dayparts: three days.
    expect(exampleDays(6, 10, 5)).toHaveLength(3);
  });

  it("says what a separation does", () => {
    expect(describeSeparation(4, 1)).toBe(
      "Played in A, it does not play in B; at the earliest in C. It can play again later in A itself: the track separation decides.",
    );
    expect(describeSeparation(4, 4)).toBe(
      "Played in A, it does not play in B, C, D, and tomorrow's A; at the earliest in tomorrow's B.",
    );
    expect(describeSeparation(4, 0)).toBe("Off: only the windows in minutes count.");
  });
});
