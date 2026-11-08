// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { placeOf } from "./pages";

describe("placeOf", () => {
  it("gives a page its section, inherited from the page above it", () => {
    expect(placeOf("/library")).toMatchObject({ section: "nav.library", nested: false });
    expect(placeOf("/medleys")).toMatchObject({ section: "nav.library", page: { label: "nav.medleys" } });
    expect(placeOf("/settings")).toMatchObject({ section: "nav.station", page: { label: "nav.settings" } });
  });

  it("knows the module of a page, inside pages too", () => {
    expect(placeOf("/medleys/4")?.page.module).toBe("medleys");
    expect(placeOf("/statistics")?.page.module).toBe("listener_statistics");
    expect(placeOf("/webhooks")?.page.module).toBe("webhooks");
    expect(placeOf("/library")?.page.module).toBeUndefined();
    expect(placeOf("/modules")).toMatchObject({ section: "nav.administration", page: { admin: true } });
    // Each show names its section: listed under it while the others are off.
    expect(placeOf("/shows/match")).toMatchObject({ section: "nav.shows", page: { module: "match" } });
    expect(placeOf("/shows/top30")).toMatchObject({ section: "nav.shows", page: { module: "top30" } });
  });

  it("lists the exact times under the rotation log", () => {
    expect(placeOf("/exact-times")).toMatchObject({
      section: "nav.programming",
      page: { label: "nav.exactTimes", station: true, sub: true },
    });
  });

  it("knows a page inside a page", () => {
    expect(placeOf("/library/12")).toMatchObject({ page: { path: "/library" }, nested: true });
    expect(placeOf("/clocks/new")).toMatchObject({ section: "nav.programming", nested: true });
  });

  it("leaves the pages above the sections, and unknown ones, without a section", () => {
    expect(placeOf("/")?.section).toBeUndefined();
    expect(placeOf("/queue")?.section).toBeUndefined();
    expect(placeOf("/account")).toBeNull();
  });
});
