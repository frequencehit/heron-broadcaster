// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent, user } from "../test/render";
import { ChartCard, Top30Page, Top30Rank } from "./Top30";

const on = { ...ALL_ON, top30: true };
const MODULES = { "GET /api/v1/stations/{id}/modules": { installation: on, station: on, on } };

const entry = (rank: number, track: number, movement: string, previous: number | null) => ({
  rank,
  track_id: track,
  artist: "Artist",
  title: `Track ${track}`,
  likes: 9,
  dislikes: 1,
  share: 0.9,
  plays: 3,
  movement,
  previous_rank: previous,
  weeks: 2,
});

const CHART = {
  week: "2026-11-02",
  from_unix_ms: Date.UTC(2026, 10, 1, 23),
  closes_at_unix_ms: Date.UTC(2026, 10, 8, 16),
  frozen: false,
  locked: false,
  votes: 120,
  entries: [entry(1, 11, "same", 1), entry(2, 12, "up", 3), entry(3, 13, "new", null)],
  removed: [
    { track_id: 14, artist: "Gone", title: "Away", removed_by: "admin", removed_at_unix_ms: Date.UTC(2026, 10, 5) },
  ],
  new_release: null,
};

describe("top 30", () => {
  beforeEach(() => chooseStation(1));

  it("shows the chart live, takes a track out and puts one back", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/top30/chart": CHART,
      "POST /api/v1/stations/{station_id}/top30/charts/{week}/removals/{track_id}": CHART,
      "DELETE /api/v1/stations/{station_id}/top30/charts/{week}/removals/{track_id}": CHART,
    });
    renderApp(<ChartCard week={null} />);
    expect(await screen.findByText("Artist – Track 12")).toBeTruthy();
    expect(screen.getByText("▲ 1")).toBeTruthy();
    expect(screen.getByText("NEW")).toBeTruthy();
    expect(screen.getByText("live")).toBeTruthy();
    fireEvent.click(screen.getAllByText("Take out")[1] as HTMLElement);
    await waitFor(() =>
      expect(sent("POST /api/v1/stations/{station_id}/top30/charts/{week}/removals/{track_id}")).toHaveLength(1),
    );
    fireEvent.click(screen.getByText("Put back"));
    await waitFor(() =>
      expect(sent("DELETE /api/v1/stations/{station_id}/top30/charts/{week}/removals/{track_id}")).toHaveLength(1),
    );
  });

  it("does not let viewers change a chart", async () => {
    answer({ ...MODULES, "GET /api/v1/stations/{station_id}/top30/chart": CHART });
    renderApp(<ChartCard week={null} />, user("viewer"));
    expect(await screen.findByText("Artist – Track 11")).toBeTruthy();
    expect(screen.queryByText("Take out")).toBeNull();
  });

  it("gives a track's rank in this week's chart", async () => {
    answer({ ...MODULES, "GET /api/v1/stations/{station_id}/top30/chart": CHART });
    renderApp(<Top30Rank trackId={12} />);
    expect(await screen.findByText("No. 2 in this week's Top 30")).toBeTruthy();
  });

  it("plans the show and lists past weeks", async () => {
    const opens = Date.UTC(2026, 10, 8, 17);
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/shows/{module}": {
        module: "top30",
        settings: {
          enabled: true,
          hours: [{ days: [7], start_hour: 18, end_hour: 20 }],
          website: { title: "", color: "#c62828", small_widget: true, closed: "message", closed_message: "", page_url: null },
          notifications: {
            webhook_ids: [],
            lead_minutes: 60,
            language: "en",
            events: {
              "show.upcoming": { enabled: true, message: "{name} opens at {next}" },
              "top.published": { enabled: true, message: "This week's number 1: {artist} – {title}" },
            },
          },
        },
        status: { open: false, closes_at_unix_ms: null, next_opening_unix_ms: opens },
        timezone: "Europe/Paris",
        webhooks: [],
      },
      "GET /api/v1/stations/{station_id}/shows": [],
      "GET /api/v1/stations/{station_id}/top30/chart": { ...CHART, new_release: { track_id: 40, artist: "North", title: "Hearts", proposed: true } },
      "GET /api/v1/stations/{station_id}/top30/settings": {
        close_day: 7,
        close_hour: 17,
        min_votes: 10,
        size: 30,
        category_ids: [],
        jingle_category_id: null,
        new_release_category_id: 5,
      },
      "GET /api/v1/tracks": { items: [], total: 0 },
      "GET /api/v1/categories": [],
      "GET /api/v1/stations/{station_id}/top30/show": {
        opens_at_unix_ms: opens,
        closes_at_unix_ms: opens + 7_200_000,
        week: "2026-11-02",
        built: false,
        items: [
          { kind: "track", rank: 2, track_id: 12, artist: "Artist", title: "Track 12", length_ms: 180_000, planned_at_unix_ms: opens, skipped: null, queued: false },
          { kind: "track", rank: 1, track_id: 11, artist: "Artist", title: "Track 11", length_ms: 180_000, planned_at_unix_ms: opens + 180_000, skipped: "restriction 3 forbids it then", queued: false },
          { kind: "new_release", rank: null, track_id: 40, artist: "North", title: "Hearts", length_ms: 200_000, planned_at_unix_ms: opens + 180_000, skipped: null, queued: false },
        ],
        left_out: [3],
        music_ms: 180_000,
        jingles_ms: 0,
        new_release_ms: 200_000,
        total_ms: 380_000,
      },
      "GET /api/v1/stations/{station_id}/top30/charts": [
        { week: "2026-10-26", from_unix_ms: 0, closes_at_unix_ms: 0, size: 30, number_one: { track_id: 11, artist: "Artist", title: "Track 11" } },
      ],
    });
    renderApp(<Top30Page />);
    fireEvent.click(await screen.findByText("The show"));
    expect(await screen.findByText("Ranks 3 are left out: the show is longer than its slot.")).toBeTruthy();
    expect(screen.getByText("passed over: restriction 3 forbids it then")).toBeTruthy();
    expect(await screen.findByText("proposed")).toBeTruthy();
    fireEvent.click(screen.getByText("Past weeks"));
    expect(await screen.findByText("Number 1: Artist – Track 11")).toBeTruthy();
    fireEvent.click(screen.getByText("Settings"));
    expect(await screen.findByText("Top 30 settings")).toBeTruthy();
    expect(screen.getAllByText("When the week closes").length).toBeGreaterThan(0);
  });
});
