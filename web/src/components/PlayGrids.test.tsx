// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Schemas } from "../api";
import type { GridDay, GridPlay, PlayGrid } from "../lib/playGrid";
import { answer, renderApp, sent } from "../test/render";
import { PlayGrids } from "./PlayGrids";

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

function play(date: string, time: string, station_id: number, source: GridPlay["source"]): GridPlay {
  return { started_at_unix_ms: Date.parse(`${date}T${time}:00Z`), date, time, station_id, source };
}

/** 30 days ending Thursday 5 November 2026: two plays on Monday 2 November, one today. */
function grid(): PlayGrid {
  const days: GridDay[] = [];
  for (let index = 0; index < 30; index++) {
    const ms = Date.UTC(2026, 9, 7) + index * 86_400_000;
    const date = new Date(ms).toISOString().slice(0, 10);
    days.push({ date, weekday: ((new Date(ms).getUTCDay() + 6) % 7) + 1, start_unix_ms: ms, end_unix_ms: ms + 86_400_000, plays: [] });
  }
  const monday = [play("2026-11-02", "08:12", 1, "rotation"), play("2026-11-02", "08:40", 2, "request")];
  const today = [play("2026-11-05", "14:58", 1, "manual")];
  (days[26] as GridDay).plays = monday;
  (days[29] as GridDay).plays = today;
  return {
    time_zone: "Europe/Paris",
    today: "2026-11-05",
    days,
    hours_from: "2026-10-07",
    hours: [
      { weekday: 1, hour: 8, plays: [...monday].reverse() },
      { weekday: 4, hour: 14, plays: today },
    ],
  };
}

const GRID = "GET /api/v1/tracks/{id}/plays/grid";

function setup() {
  answer({ "GET /api/v1/stations": [station(1, "main"), station(2, "hits80")], [GRID]: grid() });
  return renderApp(<PlayGrids trackId={42} />);
}

describe("PlayGrids", () => {
  it("draws the 30 days by week, today framed and the days around greyed out", async () => {
    const { container } = setup();
    await screen.findByText("3 plays from Oct 7 to Nov 5");
    const rows = [...container.querySelectorAll(".play-days tbody tr")];
    expect(rows).toHaveLength(5);
    expect(rows.map((row) => row.querySelector(".total")?.textContent)).toEqual(["0", "0", "0", "0", "3"]);
    expect(container.querySelectorAll(".play-days .play-day.out")).toHaveLength(5);
    const today = container.querySelector(".play-day.today");
    expect(today?.getAttribute("aria-label")).toBe("Thu, Nov 5: 1 play");
    // Today's plays are shown first; all stations: each play's station.
    expect(screen.getByText("· main, manual")).toBeTruthy();
    expect(screen.getByText(/Every station's plays added up/)).toBeTruthy();
  });

  it("gives a day's plays on hover and focus, and opens the history on a click", async () => {
    const { container } = setup();
    const monday = await screen.findByRole("button", { name: "Mon, Nov 2: 2 plays" });
    fireEvent.mouseEnter(monday);
    const detail = within(container.querySelector(".play-detail") as HTMLElement);
    expect(detail.getByText("08:12")).toBeTruthy();
    expect(detail.getByText("· hits80, request")).toBeTruthy();
    fireEvent.focus(screen.getByRole("button", { name: "Tue, Nov 3: no plays" }));
    expect(screen.getByText("Tue, Nov 3: no plays", { selector: "strong" })).toBeTruthy();
    fireEvent.click(monday);
    const day = grid().days[26] as GridDay;
    expect(window.location.pathname + window.location.search).toBe(
      `/history?track=42&after=${day.start_unix_ms}&before=${day.end_unix_ms}`,
    );
  });

  it("ticks the hours of the week the track played in, with their dates", async () => {
    const { container } = setup();
    await screen.findByText("3 plays from Oct 7 to Nov 5");
    const ticked = [...container.querySelectorAll(".play-hour")].filter((cell) => cell.textContent === "✓");
    expect(ticked).toHaveLength(2);
    expect(container.querySelectorAll(".play-hour")).toHaveLength(24 * 7);
    // The first ticked cell is shown at first.
    expect(screen.getByText("Mon 08:00–08:59: 2 plays in 30 days", { selector: "strong" })).toBeTruthy();
    fireEvent.mouseEnter(screen.getByRole("button", { name: "Thu 14:00–14:59: 1 play in 30 days" }));
    expect(screen.getByText("Thu 14:00–14:59: 1 play in 30 days", { selector: "strong" })).toBeTruthy();
    fireEvent.focus(screen.getByRole("button", { name: "Sun 02:00–02:59: no plays in 30 days" }));
    expect(screen.getByText("Sun 02:00–02:59: no plays in 30 days", { selector: "strong" })).toBeTruthy();
  });

  it("asks for one station, the rotation only and a longer period", async () => {
    setup();
    await screen.findByText("3 plays from Oct 7 to Nov 5");
    expect(sent(GRID)[0]?.options).toEqual({ params: { path: { id: 42 }, query: { days: 30 } } });
    fireEvent.click(screen.getByRole("button", { name: "HITS80 (hits80)" }));
    fireEvent.click(screen.getByRole("button", { name: "Rotation only" }));
    fireEvent.click(screen.getByRole("button", { name: "365 days" }));
    await waitFor(() =>
      expect(sent(GRID).at(-1)?.options).toEqual({
        params: { path: { id: 42 }, query: { station_id: 2, source: "rotation", days: 365 } },
      }),
    );
    expect(await screen.findByText(/in the station's time zone/)).toBeTruthy();
  });
});
