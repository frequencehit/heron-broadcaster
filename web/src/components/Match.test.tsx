// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent, user } from "../test/render";
import { CurrentMatch, History, MatchOnAir, SettingsCard } from "./Match";

const on = { ...ALL_ON, match: true };
const MODULES = { "GET /api/v1/stations/{id}/modules": { installation: on, station: on, on } };
const SETTINGS = {
  every_tracks: 4,
  tracks_from: "category",
  announce_every: 1,
  category_id: 3,
  jingle_category_id: null,
  close_before_s: 10,
  live_results: true,
  loser_same_day: false,
};

const match = (more: Record<string, unknown> = {}) => ({
  id: 7,
  status: "open",
  category_id: 3,
  category_name: "Hits",
  vote_track: { track_id: 1, artist: "Saltwater Choir", title: "Lighthouse Keeper" },
  jingle: { track_id: 9, artist: null, title: "Vote now" },
  choices: [
    { choice: "a", track_id: 2, artist: "Cassia Bloom", title: "Neon Orchard", votes: 46, percent: 58 },
    { choice: "b", track_id: 3, artist: "Echo Parade", title: "Running Lights", votes: 33, percent: 42 },
  ],
  votes: 79,
  opened_at_unix_ms: Date.now() - 60_000,
  closes_at_unix_ms: Date.now() + 72_000,
  transition_at_unix_ms: Date.now() + 82_000,
  winner: null,
  decided_by: null,
  closed_at_unix_ms: null,
  ...more,
});

describe("match", () => {
  beforeEach(() => chooseStation(1));

  it("shows the match in progress, closes its vote and cancels it", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/current": match(),
      "GET /api/v1/stations/{station_id}/match/settings": SETTINGS,
      "POST /api/v1/stations/{station_id}/match/current/close": match({ status: "closed", winner: "a" }),
      "POST /api/v1/stations/{station_id}/match/current/cancel": match({ status: "cancelled" }),
    });
    renderApp(<CurrentMatch empty />);
    expect(await screen.findByText("Cassia Bloom – Neon Orchard")).toBeTruthy();
    expect(screen.getByText("58 %")).toBeTruthy();
    expect(screen.getByText("33 votes")).toBeTruthy();
    expect(screen.getByText("During: Saltwater Choir – Lighthouse Keeper")).toBeTruthy();
    expect(screen.getByText("Announcement over the intro")).toBeTruthy();
    expect(await screen.findByText("every 4 tracks")).toBeTruthy();
    fireEvent.click(screen.getByText("Close the vote now"));
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/match/current/close")).toHaveLength(1));
    fireEvent.click(screen.getByText("Cancel the match"));
    fireEvent.click((await screen.findAllByText("Cancel the match"))[1] as HTMLElement);
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/match/current/cancel")).toHaveLength(1));
  });

  it("says why no match runs, and lets viewers only look", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/current": null,
      "GET /api/v1/stations/{station_id}/match/settings": { ...SETTINGS, category_id: null },
    });
    renderApp(<CurrentMatch empty />, user("viewer"));
    expect(await screen.findByText(/No match runs until the category/)).toBeTruthy();
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/current": match({ status: "queued", opened_at_unix_ms: null }),
      "GET /api/v1/stations/{station_id}/match/settings": SETTINGS,
    });
    renderApp(<MatchOnAir />, user("viewer"));
    expect(await screen.findByText("Waiting for its track to start")).toBeTruthy();
    expect(screen.queryByText("Close the vote now")).toBeNull();
  });

  it("lists the matches played", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/history": {
        matches: [
          match({ id: 2, status: "closed", winner: "a", decided_by: "votes", closed_at_unix_ms: Date.now() }),
          match({ id: 1, status: "closed", winner: "b", decided_by: "no_votes", votes: 0 }),
        ],
        week: { from_unix_ms: 0, matches: 28, votes: 2104 },
      },
    });
    renderApp(<History />);
    expect(await screen.findByText("Matches played")).toBeTruthy();
    expect(screen.getByText("no vote")).toBeTruthy();
    expect(screen.getByText(/28 matches/)).toBeTruthy();
  });

  it("says every track on air, and a match from the clock needs no category", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/current": match({ status: "queued", opened_at_unix_ms: null }),
      "GET /api/v1/stations/{station_id}/match/settings": { ...SETTINGS, every_tracks: 1 },
    });
    renderApp(<MatchOnAir />, user("viewer"));
    expect(await screen.findByText("every track")).toBeTruthy();
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/current": null,
      "GET /api/v1/stations/{station_id}/match/settings": { ...SETTINGS, tracks_from: "clock", category_id: null },
    });
    renderApp(<CurrentMatch empty />, user("viewer"));
    expect(await screen.findByText(/The next one comes/)).toBeTruthy();
  });

  it("takes the tracks from the clock, every track, announced every 2 matches", async () => {
    chooseStation(1);
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/match/settings": SETTINGS,
      "GET /api/v1/categories": [{ id: 3, name: "Hits", station_ids: [1] }],
      "PUT /api/v1/stations/{station_id}/match/settings": (options: { body: unknown }) => options.body,
    });
    renderApp(<SettingsCard />);
    fireEvent.click(await screen.findByRole("button", { name: "From the clock" }));
    expect(await screen.findByText("In the hours without a clock")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("A match every"), { target: { value: "1" } });
    fireEvent.change(screen.getByLabelText("The announcement every"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent("PUT /api/v1/stations/{station_id}/match/settings")).toHaveLength(1));
    expect(sent("PUT /api/v1/stations/{station_id}/match/settings")[0]?.options).toMatchObject({
      body: { tracks_from: "clock", every_tracks: 1, announce_every: 2, category_id: 3 },
    });
  });
});
