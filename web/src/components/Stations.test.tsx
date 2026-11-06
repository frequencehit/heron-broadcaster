// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { Schemas } from "../api";
import { chooseStation, initialStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { Medleys } from "../pages/Medleys";
import { StationFilter, useStationFilter } from "./Stations";

function station(id: number, code: string): Schemas["StationInfo"] {
  return {
    id,
    code,
    name: code.toUpperCase(),
    enabled: true,
    engine_configured: true,
    engine_variable: "HERON_ENGINE_URL",
    position: id,
  };
}

const TWO = [station(1, "main"), station(2, "hits80")];

function Harness() {
  const filter = useStationFilter("tracks");
  return (
    <>
      <StationFilter filter={filter} />
      <output>{filter.stationId ?? "every station"}</output>
    </>
  );
}

beforeEach(() => {
  localStorage.clear();
  initialStation(1, TWO);
  chooseStation(2);
});

describe("StationFilter", () => {
  it("starts on the switcher's station, and keeps another choice per page", async () => {
    answer({ "GET /api/v1/stations": TWO });
    const { unmount } = renderApp(<Harness />);
    const select = (await screen.findByRole("combobox", { name: "Station" })) as HTMLSelectElement;
    expect(select.value).toBe("2");
    expect(screen.getByRole("status").textContent).toBe("2");
    fireEvent.change(select, { target: { value: "all" } });
    expect(screen.getByRole("status").textContent).toBe("every station");
    unmount();

    // Kept for the page, whatever the switcher shows.
    chooseStation(1);
    renderApp(<Harness />);
    const again = (await screen.findByRole("combobox", { name: "Station" })) as HTMLSelectElement;
    expect(again.value).toBe("all");
    expect(localStorage.getItem("heron.stationFilter.tracks.1")).toBe("all");
    expect(localStorage.getItem("heron.stationFilter.medleys.1")).toBeNull();
  });

  it("is not shown with one station", async () => {
    answer({ "GET /api/v1/stations": [station(1, "main")] });
    renderApp(<Harness />);
    await screen.findByText("every station");
    expect(screen.queryByRole("combobox")).toBeNull();
  });
});

describe("Medleys", () => {
  it("lists the medleys of the filter's station", async () => {
    answer({ "GET /api/v1/stations": TWO, "GET /api/v1/medleys": [] });
    renderApp(<Medleys />);
    const select = await screen.findByRole("combobox", { name: "Station" });
    await screen.findByText("No medley yet.");
    const last = () => sent("GET /api/v1/medleys").at(-1)?.options as { params: { query: object } };
    expect(last().params.query).toEqual({ station_id: 2 });
    fireEvent.change(select, { target: { value: "all" } });
    await screen.findByText("No medley yet.");
    expect(last().params.query).toEqual({});
  });
});
