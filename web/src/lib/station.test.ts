// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { User } from "../api";
import { codes, firstStation, onStation, perStation, stationRole, toStation } from "./station";

describe("perStation", () => {
  it("knows a station's routes, as the API does", () => {
    expect(perStation("queue")).toBe(true);
    expect(perStation("queue/12/position")).toBe(true);
    expect(perStation("settings/medleys")).toBe(true);
    expect(perStation("schedules/3")).toBe(true);
    expect(perStation("webhook-deliveries/3/retry")).toBe(true);
    expect(perStation("listeners/days?format=csv")).toBe(true);
    expect(perStation("settings/library")).toBe(false);
    expect(perStation("tracks/1")).toBe(false);
    expect(perStation("stations")).toBe(false);
  });
});

describe("onStation", () => {
  it("sends a station's routes to the chosen station", () => {
    expect(onStation("http://h/api/v1/queue", 2)).toBe("http://h/api/v1/stations/2/queue");
    expect(onStation("/api/v1/listeners/days?format=csv", 3)).toBe("/api/v1/stations/3/listeners/days?format=csv");
    expect(onStation("ws://h/api/v1/live", 2)).toBe("ws://h/api/v1/stations/2/live");
  });

  it("leaves shared routes, and routes already on a station, alone", () => {
    expect(onStation("http://h/api/v1/tracks", 2)).toBe("http://h/api/v1/tracks");
    expect(onStation("http://h/api/v1/stations/1/queue", 2)).toBe("http://h/api/v1/stations/1/queue");
    expect(onStation("http://h/api/v1/queue", null)).toBe("http://h/api/v1/queue");
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

describe("toStation", () => {
  it("keeps the method, headers and body of what it sends to a station", async () => {
    const request = new Request("http://heron/api/v1/outputs/3", {
      method: "PUT",
      headers: { "X-Requested-With": "heron-web", "Content-Type": "application/json" },
      body: JSON.stringify({ enabled: false }),
    });
    const sent = await toStation(request, 2);
    expect(sent?.url).toBe("http://heron/api/v1/stations/2/outputs/3");
    expect(sent?.method).toBe("PUT");
    expect(sent?.headers.get("x-requested-with")).toBe("heron-web");
    expect(await sent?.json()).toEqual({ enabled: false });
  });

  it("leaves the shared routes alone", async () => {
    const request = new Request("http://heron/api/v1/tracks", { method: "POST", body: "{}" });
    expect(await toStation(request, 2)).toBeUndefined();
  });
});
