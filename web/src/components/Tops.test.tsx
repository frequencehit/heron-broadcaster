// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { newTopSettings } from "../lib/tops";
import { answer, renderApp, sent, user } from "../test/render";
import { ChartCard, TopPage, TopRanks, TopsPage } from "./Tops";

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
  plays: 20 - rank,
  plays_share: (20 - rank) / 19,
  score: 0.5 * ((20 - rank) / 19) + 0.45,
  movement,
  previous_rank: previous,
  charts: 2,
});

const CHART = {
  key: "2026-11-08",
  from_unix_ms: Date.UTC(2026, 10, 1, 19),
  to_unix_ms: Date.UTC(2026, 10, 8, 19),
  frozen: false,
  locked: false,
  votes: 120,
  max_plays: 19,
  plays_percent: 50,
  min_votes: 5,
  size: 3,
  entries: [entry(1, 11, "same", 1), entry(2, 12, "up", 3), entry(3, 13, "new", null)],
  runners_up: [entry(4, 15, "down", 2)],
  not_enough_votes: [
    {
      track_id: 16,
      artist: "Artist",
      title: "Track 16",
      plays: 19,
      plays_share: 1,
      likes: 3,
      dislikes: 0,
      share: 1,
      score: 1,
      would_rank: 1,
    },
  ],
  removed: [{ track_id: 14, artist: "Gone", title: "Away", removed_by: "admin", removed_at_unix_ms: Date.UTC(2026, 10, 5) }],
  new_release: null,
};

const SLOT = { every: "day" as const, weekday: null, nth: null, start: "20:00", end: "21:00" };

const top = (id: number, change: Record<string, unknown> = {}, gives_way: unknown[] = []) => ({
  id,
  position: id,
  key: `top-${id}`,
  settings: {
    ...newTopSettings("Top 3 of the evening"),
    size: 3,
    plays_percent: 50,
    min_votes: 5,
    period: "rolling_7_days",
    slot: SLOT,
    notifications: {
      webhook_ids: [],
      lead_minutes: 60,
      language: "en",
      events: { "top.published": { enabled: true, message: "{name}: number 1 is {artist} – {title}" } },
    },
    ...change,
  },
  error: null,
  status: { open: false, closes_at_unix_ms: null, next_opening_unix_ms: Date.UTC(2026, 10, 8, 19) },
  next_show: { opens_at_unix_ms: Date.UTC(2026, 10, 8, 19), closes_at_unix_ms: Date.UTC(2026, 10, 8, 20) },
  gives_way,
  timezone: "Europe/Paris",
  webhooks: [],
});

describe("tops", () => {
  beforeEach(() => {
    chooseStation(1);
    window.history.replaceState(null, "", "/shows/top30");
  });

  it("lists the tops with their period, score and show, and the one giving way", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/tops": [
        top(1),
        top(2, { name: "Top 30", size: 30, period: "week", plays_percent: 0, min_votes: 10, slot: { every: "week", weekday: 7, nth: null, start: "18:00", end: "20:00" } }, [
          { top_id: 3, name: "Top 30 of the month", slot: { every: "month", weekday: 7, nth: "first", start: "18:00", end: "20:00" } },
        ]),
      ],
      "POST /api/v1/stations/{station_id}/tops": top(9),
    });
    renderApp(<TopsPage />);
    expect(await screen.findByText("Top 3 of the evening")).toBeTruthy();
    expect(screen.getByText("The last 7 days, frozen before each show")).toBeTruthy();
    expect(screen.getByText("50 % plays · 50 % likes · 5 votes min.")).toBeTruthy();
    expect(screen.getByText("Every day, 20:00–21:00")).toBeTruthy();
    expect(screen.getByText("The week, reset on Sunday at 17:00")).toBeTruthy();
    expect(
      screen.getByText("1st Sunday of the month, “Top 30 of the month” takes the same slot: this one gives way that day."),
    ).toBeTruthy();
    fireEvent.click(screen.getByText("+ New top"));
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/tops")).toHaveLength(1));
    const body = sent("POST /api/v1/stations/{station_id}/tops")[0]?.options?.body as { name: string; size: number };
    expect(body.name).toBe("New top");
  });

  it("shows the live chart: plays, likes, the score in two parts, the ranks after the size and the votes missing", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/tops/{top_id}/chart": CHART,
      "POST /api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/removals/{track_id}": CHART,
      "DELETE /api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/removals/{track_id}": CHART,
    });
    renderApp(<ChartCard top={top(1) as never} chartKey={null} />);
    expect(await screen.findByText("Artist – Track 12")).toBeTruthy();
    expect(screen.getByText("▲ 1")).toBeTruthy();
    expect(screen.getByText("NEW")).toBeTruthy();
    expect(screen.getByText("live")).toBeTruthy();
    expect(screen.getByText("Frozen just before each show", { exact: false })).toBeTruthy();
    // Number 4 is outside the Top 3: greyed, not taken out.
    const outside = screen.getByText("Artist – Track 15").closest("tr");
    expect(outside?.className).toBe("top-outside");
    expect(screen.getAllByText("Take out")).toHaveLength(3);
    expect(screen.getByText("The greyed ranks are outside the Top 3: they show who is just behind.")).toBeTruthy();
    expect(screen.getByText("Not enough votes (fewer than 5)")).toBeTruthy();
    expect(screen.getByText("its score would make it no. 1", { exact: false })).toBeTruthy();
    fireEvent.click(screen.getAllByText("Take out")[1] as HTMLElement);
    await waitFor(() =>
      expect(sent("POST /api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/removals/{track_id}")).toHaveLength(1),
    );
    fireEvent.click(screen.getByText("Put back"));
    await waitFor(() =>
      expect(sent("DELETE /api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/removals/{track_id}")).toHaveLength(1),
    );
  });

  it("does not let viewers change a chart", async () => {
    answer({ ...MODULES, "GET /api/v1/stations/{station_id}/tops/{top_id}/chart": CHART });
    renderApp(<ChartCard top={top(1) as never} chartKey={null} />, user("viewer"));
    expect(await screen.findByText("Artist – Track 11")).toBeTruthy();
    expect(screen.queryByText("Take out")).toBeNull();
  });

  it("gives a track's rank in each top's live chart", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/tops": [top(1)],
      "GET /api/v1/stations/{station_id}/tops/{top_id}/chart": CHART,
    });
    renderApp(<TopRanks trackId={12} />);
    expect(await screen.findByText("No. 2 in Top 3 of the evening")).toBeTruthy();
  });

  it("plans the show, lists past charts and saves the settings", async () => {
    const opens = Date.UTC(2026, 10, 8, 19);
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/tops/{top_id}": top(1, { new_release_category_id: 5 }),
      "PUT /api/v1/stations/{station_id}/tops/{top_id}": top(1),
      "GET /api/v1/stations/{station_id}/tops/{top_id}/chart": {
        ...CHART,
        new_release: { track_id: 40, artist: "North", title: "Hearts", proposed: true },
      },
      "GET /api/v1/stations/{station_id}/settings/station": { timezone: "Europe/Paris", public_url: null, gateway_url: null },
      "GET /api/v1/tracks": { items: [], total: 0 },
      "GET /api/v1/categories": [],
      "GET /api/v1/stations/{station_id}/tops/{top_id}/show": {
        opens_at_unix_ms: opens,
        closes_at_unix_ms: opens + 3_600_000,
        key: "2026-11-08",
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
      "GET /api/v1/stations/{station_id}/tops/{top_id}/charts": [
        { key: "2026-11-01", from_unix_ms: 0, to_unix_ms: 0, size: 3, number_one: { track_id: 11, artist: "Artist", title: "Track 11" } },
      ],
    });
    renderApp(<TopPage id={1} />);
    fireEvent.click(await screen.findByText("The show"));
    expect(await screen.findByText("Ranks 3 are left out: the show is longer than its slot.")).toBeTruthy();
    expect(screen.getByText("passed over: restriction 3 forbids it then")).toBeTruthy();
    expect(await screen.findByText("proposed")).toBeTruthy();
    fireEvent.click(screen.getByText("Past charts"));
    expect(await screen.findByText("Number 1: Artist – Track 11")).toBeTruthy();
    fireEvent.click(screen.getByText("Settings"));
    expect(await screen.findByText("The period")).toBeTruthy();
    expect(screen.getByText("Next:", { exact: false })).toBeTruthy();
    expect(screen.getByText("score = 50 % × plays + 50 % × share of likes")).toBeTruthy();
    expect(screen.getAllByText("When a chart is frozen").length).toBeGreaterThan(0);
    // Top 10, 70 % plays, the 1st Saturday and Sunday of the month.
    fireEvent.click(screen.getByText("Top 10"));
    fireEvent.change(screen.getByLabelText("Plays or votes"), { target: { value: "30" } });
    expect(screen.getByText("score = 70 % × plays + 30 % × share of likes")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("When"), { target: { value: "first" } });
    expect(screen.getByText("of the month")).toBeTruthy();
    // Every day saved before: the seven days pressed, five taken off.
    expect(screen.getByLabelText("Sunday").getAttribute("aria-pressed")).toBe("true");
    for (const day of ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"]) {
      fireEvent.click(screen.getByLabelText(day));
    }
    expect(screen.getByLabelText("Monday").getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() => expect(sent("PUT /api/v1/stations/{station_id}/tops/{top_id}")).toHaveLength(1));
    const body = sent("PUT /api/v1/stations/{station_id}/tops/{top_id}")[0]?.options?.body as {
      size: number;
      plays_percent: number;
      slot: { every: string; nth: string; weekdays: number[]; weekday: number | null };
    };
    expect(body.size).toBe(10);
    expect(body.plays_percent).toBe(70);
    expect(body.slot).toMatchObject({ every: "month", nth: "first", weekdays: [6, 7], weekday: null });
  });
});
