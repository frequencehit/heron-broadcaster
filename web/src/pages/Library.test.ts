// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { filtersOf } from "./Library";

describe("filtersOf", () => {
  it("lists the tracks on the filter's station", () => {
    expect(filtersOf(new URLSearchParams("category=3&station=1"), "", 2)).toEqual({
      category_id: 3,
      on_station_id: 2,
      station_id: 1,
    });
    expect(filtersOf(new URLSearchParams(""), "", undefined)).toEqual({});
  });

  it("shows the tracks in no category whatever the station", () => {
    expect(filtersOf(new URLSearchParams("category=none"), "", 2)).toEqual({ no_category: true });
  });
});
