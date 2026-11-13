// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent, user } from "../test/render";
import { PlanningCard, SettingsCard, TodayCard } from "./FeaturedArtist";

const on = { ...ALL_ON, featured_artist: true };
const MODULES = { "GET /api/v1/stations/{id}/modules": { installation: on, station: on, on } };
const SETTINGS = { minutes: [0], by: "week", window_before_s: 10, window_after_s: 30, jingle_category_id: null };
const PLANNING = {
  by: "week",
  entries: [
    { date: "2026-11-09", artist_id: 4, artist: "Mira Solano", current: true },
    { date: "2026-11-16", artist_id: null, artist: null, current: false },
  ],
};
const ARTIST = { id: 8, name: "Cassia Bloom", names: [{ name: "Cassia Bloom" }], tracks: 12, musicbrainz_id: null };

describe("featured artist", () => {
  beforeEach(() => chooseStation(1));

  it("plans an artist per week and takes one out", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/featured-artist/planning": PLANNING,
      "GET /api/v1/artists": { artists: [ARTIST], total: 1 },
      "PUT /api/v1/stations/{station_id}/featured-artist/planning/{date}": {
        date: "2026-11-16",
        artist_id: 8,
        artist: "Cassia Bloom",
        current: false,
      },
      "DELETE /api/v1/stations/{station_id}/featured-artist/planning/{date}": null,
    });
    renderApp(<PlanningCard />);
    expect(await screen.findByText("Mira Solano")).toBeTruthy();
    expect(screen.getByText("This week")).toBeTruthy();
    expect(screen.getByText("No artist")).toBeTruthy();
    expect(screen.getByText("1 without an artist")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Choose" }));
    fireEvent.change(screen.getByPlaceholderText(/artist/i), { target: { value: "cass" } });
    expect(await screen.findByText("Cassia Bloom")).toBeTruthy();
    const choose = screen.getAllByRole("button", { name: "Choose" });
    fireEvent.click(choose[choose.length - 1] as HTMLElement);
    await waitFor(() =>
      expect(sent("PUT /api/v1/stations/{station_id}/featured-artist/planning/{date}")[0]?.options).toMatchObject({
        params: { path: { station_id: 1, date: "2026-11-16" } },
        body: { artist_id: 8 },
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click((await screen.findAllByRole("button", { name: "Remove" }))[1] as HTMLElement);
    await waitFor(() =>
      expect(sent("DELETE /api/v1/stations/{station_id}/featured-artist/planning/{date}")).toHaveLength(1),
    );
  });

  it("lists today's times with what plays at each", async () => {
    const at = Date.UTC(2026, 10, 13, 12, 0);
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/featured-artist/day": {
        date: "2026-11-13",
        artist_id: 4,
        artist: "Mira Solano",
        times: [
          {
            at_unix_ms: at,
            status: "played",
            reason: null,
            jingle: { track_id: 9, artist: null, title: "Featured artist", length_ms: 4000 },
            track: { track_id: 2, artist: "Mira Solano", title: "Sunday Market", length_ms: 200000 },
            started_at_unix_ms: at + 3000,
          },
          { at_unix_ms: at + 3_600_000, status: "passed_over", reason: "nothing could play", jingle: null, track: null },
          {
            at_unix_ms: at + 7_200_000,
            status: "planned",
            reason: null,
            jingle: null,
            track: { track_id: 3, artist: "Mira Solano", title: "Harbour Lights", length_ms: 210000 },
          },
        ],
      },
    });
    renderApp(<TodayCard />);
    expect(await screen.findByText("Today: Mira Solano")).toBeTruthy();
    expect(screen.getByText("Mira Solano – Sunday Market")).toBeTruthy();
    expect(screen.getByText("Played")).toBeTruthy();
    expect(screen.getByText("Passed over")).toBeTruthy();
    expect(screen.getByText(/Nothing could play then/)).toBeTruthy();
    expect(screen.getByText("Planned")).toBeTruthy();
  });

  it("saves its minutes, by day, and lets viewers only look", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/featured-artist/settings": SETTINGS,
      "GET /api/v1/categories": [],
      "PUT /api/v1/stations/{station_id}/featured-artist/settings": (options: { body: unknown }) => options.body,
    });
    renderApp(<SettingsCard />);
    fireEvent.click(await screen.findByRole("checkbox", { name: ":15" }));
    fireEvent.click(screen.getByRole("checkbox", { name: ":45" }));
    fireEvent.click(screen.getByRole("button", { name: "Per day" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(sent("PUT /api/v1/stations/{station_id}/featured-artist/settings")[0]?.options).toMatchObject({
        body: { minutes: [0, 15, 45], by: "day" },
      }),
    );
    // No minute: nothing to save.
    for (const minute of [":00", ":15", ":45"]) {
      fireEvent.click(screen.getByRole("checkbox", { name: minute }));
    }
    expect(screen.getByText("Choose one minute at least.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows viewers the settings without changing them", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/featured-artist/settings": SETTINGS,
      "GET /api/v1/categories": [],
    });
    renderApp(<SettingsCard />, user("viewer"));
    const minute = (await screen.findByRole("checkbox", { name: ":00" })) as HTMLInputElement;
    expect(minute.checked).toBe(true);
    expect(minute.disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });
});
