// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { User } from "../api";
import { chooseStation, codes, firstStation, here, stationRole, stationUrl } from "./station";

describe("here", () => {
  it("adds the station the pages show to a route's path parameters", () => {
    chooseStation(null);
    expect(() => here()).toThrow();
    chooseStation(2);
    expect(here()).toEqual({ station_id: 2 });
    expect(here({ id: 5 })).toEqual({ id: 5, station_id: 2 });
  });
});

describe("stationUrl", () => {
  it("is a route under the station, or the first station's without one", () => {
    chooseStation(3);
    expect(stationUrl("live")).toBe("/api/v1/stations/3/live");
    expect(stationUrl("listeners/days?format=csv")).toBe("/api/v1/stations/3/listeners/days?format=csv");
    expect(stationUrl("live", null)).toBe("/api/v1/live");
  });
});

describe("stations", () => {
  const station = (id: number, code: string) => ({
    id,
    code,
    name: code,
    engine_variable: "HERON_ENGINE_URL", engine_configured: true,
    enabled: true,
    position: 0,
  });

  it("finds the first station and the codes of badges", () => {
    const stations = [station(3, "hits80"), station(1, "main")];
    expect(firstStation(stations)?.code).toBe("main");
    expect(codes([1], stations)).toEqual(["main"]);
  });

  it("gives the role on a station; a shared admin is admin everywhere", () => {
    const user = {
      role: "viewer",
      station_roles: [{ station_id: 2, role: "operator" }],
    } as unknown as User;
    expect(stationRole(user, 2)).toBe("operator");
    expect(stationRole(user, 1)).toBeUndefined();
    expect(stationRole(user, null)).toBe("viewer");
    expect(stationRole({ ...user, role: "admin" }, 1)).toBe("admin");
  });
});
