// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Schemas } from "../api";
import { type PlainKey, t } from "../i18n";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { Statistics } from "./Statistics";

type Audience = Schemas["Audience"];

const LENGTHS = { under_1_min: 2, from_1_to_5_min: 2, from_5_to_30_min: 4, from_30_min_to_2_h: 1, over_2_h: 1 };

const CURRENT: Audience = {
  from: "2026-10-08",
  to: "2026-11-06",
  days: 30,
  peak: 42,
  peak_at_unix_ms: Date.UTC(2026, 10, 2, 8, 15),
  sessions: 10,
  ttsl_hours: 12.5,
  atsl_s: 4_500,
  median_session_s: 900,
  cume: 6,
  listener_days: 180,
  aqh: 3.25,
  session_lengths: LENGTHS,
};
const PREVIOUS: Audience = { ...CURRENT, from: "2026-09-08", to: "2026-10-07", sessions: 8, cume: 8, peak: 42 };

const ROUTES = ["audience", "days", "hours-of-day", "tracks", "breakdown"] as const;

function show(
  search: string,
  audience: { current: Audience; previous: Audience } = { current: CURRENT, previous: PREVIOUS },
  more: Record<string, unknown> = {},
) {
  window.history.replaceState(null, "", `/statistics${search}`);
  chooseStation(1);
  answer({
    "GET /api/v1/stations/{id}/modules": { on: { medleys: true, cover_art: true, listener_statistics: true, webhooks: true } },
    "GET /api/v1/stations/{station_id}/settings/station": { timezone: "UTC", public_url: null },
    "GET /api/v1/stations/{station_id}/outputs": [],
    "GET /api/v1/stations/{station_id}/listeners/now": {
      outputs: [],
      total: 5,
      servers: [
        { server_id: 1, name: "Main", relay_of: null, listeners: 3, error: null },
        { server_id: 2, name: "Edge", relay_of: 1, listeners: 2, error: null },
      ],
    },
    "GET /api/v1/stations/{station_id}/listeners/samples": [],
    "GET /api/v1/stations/{station_id}/listeners/audience": audience,
    "GET /api/v1/stations/{station_id}/listeners/days": (options: unknown) =>
      (options as { params: { query: { group?: string } } }).params.query.group === "hour"
        ? [
            // Monday 2 November, 8:00.
            { day: "2026-11-02", hour: 8, days: 1, minutes: 60, peak: 9, average: 6, listening_hours: 6, sessions: null, average_session_s: null, distinct_listeners: null, listener_days: null },
          ]
        : [],
    "GET /api/v1/stations/{station_id}/listeners/hours-of-day": [
      { hour: 8, days: 30, average: 4, peak: 9, listening_hours: 4, tune_in: 1.5, tune_out: 0.5 },
    ],
    "GET /api/v1/stations/{station_id}/listeners/tracks": [],
    "GET /api/v1/stations/{station_id}/listeners/breakdown": (options: unknown) => {
      const by = (options as { params: { query: { by: string } } }).params.query.by;
      const group = (kind: "named" | "unknown" | "other", name: string | null, share: number) => ({
        kind,
        name,
        listeners: Math.round(share * 20),
        listener_share: share,
        sessions: 3,
        ttsl_hours: 1,
        ttsl_share: share,
      });
      if (by === "country") {
        return { geoip: "DBIP-Country-Lite", groups: [group("named", "FR", 0.6), group("unknown", null, 0.25), group("other", null, 0.15)] };
      }
      if (by === "device") {
        return { geoip: "DBIP-Country-Lite", groups: [group("named", "smart_speaker", 0.7), group("other", null, 0.3)] };
      }
      return { geoip: "DBIP-Country-Lite", groups: [group("named", "VLC", 1)] };
    },
    ...more,
  });
  renderApp(<Statistics />);
}

function query(route: string): Record<string, unknown> | undefined {
  const call = sent(`GET /api/v1/stations/{station_id}/listeners/${route}`).at(-1);
  return (call?.options as { params?: { query?: Record<string, unknown> } } | undefined)?.params?.query;
}

describe("Statistics", () => {
  beforeEach(() => {
    // jsdom has no layout: the charts keep their default width.
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
      },
    );
  });

  it("limits every card to the server of the address", async () => {
    show("?server=2");
    await waitFor(() => expect(query("breakdown")).toBeTruthy());
    for (const route of ROUTES) {
      await waitFor(() => expect(query(route)).toMatchObject({ server_id: 2 }));
    }
    expect(query("samples")).toMatchObject({ server_id: 2 });
    const select = (await screen.findByLabelText(t("statistics.server"))) as HTMLSelectElement;
    expect(select.value).toBe("2");
    // The CSV downloads follow it too.
    const csv = screen.getAllByRole("link", { name: t("statistics.csv") }).map((a) => a.getAttribute("href") ?? "");
    expect(csv.length).toBeGreaterThan(0);
    expect(csv.every((href) => href.includes("server_id=2"))).toBe(true);
  });

  it("keeps the server chosen in the address", async () => {
    show("");
    const select = await screen.findByLabelText(t("statistics.server"));
    fireEvent.change(select, { target: { value: "1" } });
    await waitFor(() => expect(window.location.search).toBe("?server=1"));
    await waitFor(() => expect(query("audience")).toMatchObject({ server_id: 1 }));
    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() => expect(window.location.search).toBe(""));
  });

  it("shows the audience figures against the previous period", async () => {
    show("");
    expect(await screen.findByRole("button", { name: t("statistics.about", { name: t("statistics.cume") }) })).toBeTruthy();
    // 10 sessions against 8; a cume of 6 against 8.
    expect(screen.getByText(t("statistics.vsPrevious", { change: "+25%" }))).toBeTruthy();
    expect(screen.getByText(t("statistics.vsPrevious", { change: "-25%" }))).toBeTruthy();
    expect(screen.getAllByText(t("statistics.vsPrevious", { change: "0%" })).length).toBeGreaterThan(0);
    expect(screen.getByRole("img", { name: t("statistics.sessionLengths") })).toBeTruthy();
    // Tune-in and tune-out on the hours of the day.
    fireEvent.click(screen.getByRole("button", { name: t("statistics.tuneIn") }));
    expect(await screen.findByText(t("statistics.tuneHelp"))).toBeTruthy();
  });

  it("explains each figure, by its plain name first", async () => {
    show("");
    const tips: [PlainKey, PlainKey][] = [
      ["statistics.cume", "statistics.cumeTip"],
      ["statistics.ttsl", "statistics.ttslHint"],
      ["statistics.atsl", "statistics.atslHint"],
      ["statistics.aqh", "statistics.aqhHint"],
      ["statistics.peakAudience", "statistics.peakTip"],
      ["statistics.sessions", "statistics.sessionsTip"],
      ["statistics.sessionLengths", "statistics.lengthsTip"],
    ];
    for (const [label, tip] of tips) {
      // Not an acronym alone: a plain name, the acronym after it.
      expect(t(label)).toMatch(/^[^(]{6,}/);
      const button = await screen.findByRole("button", { name: t("statistics.about", { name: t(label) }), description: t(tip) });
      // A tap opens it too, for phones.
      fireEvent.click(button);
      expect(button.getAttribute("aria-expanded")).toBe("true");
      fireEvent.keyDown(button, { key: "Escape" });
      expect(button.getAttribute("aria-expanded")).toBe("false");
    }
    expect(screen.getByRole("button", { description: t("statistics.tuneTip") })).toBeTruthy();
    // How the figures are counted, at the bottom.
    const how = screen.getByText(t("statistics.howCounted"));
    expect(how.closest("details")).toBeTruthy();
    expect(screen.getByText(t("statistics.howShort"))).toBeTruthy();
    expect(screen.getByText(t("statistics.howQuarters", { zone: "UTC" }))).toBeTruthy();
  });

  it("reads like a report over the period of the address", async () => {
    show("?days=7");
    const period = (await screen.findByLabelText(t("statistics.range"))) as HTMLSelectElement;
    expect(period.value).toBe("7");
    // Today's card asks for today only: the others for the period.
    for (const route of ROUTES.filter((r) => r !== "days")) {
      await waitFor(() => expect(query(route)).toMatchObject({ to: expect.any(String) as string }));
      const asked = query(route) as { from: string; to: string };
      expect((Date.parse(asked.to) - Date.parse(asked.from)) / 86_400_000).toBe(6);
    }
    // The heatmap, the countries with their credit, devices and players.
    expect(await screen.findByRole("img", { name: t("statistics.heatmap") })).toBeTruthy();
    expect(screen.getByTitle(t("statistics.heatmapCell", { day: "Mon", hour: "08h", value: "6" }))).toBeTruthy();
    expect(await screen.findByText("France")).toBeTruthy();
    expect(screen.getByText(t("statistics.unknownCountry"))).toBeTruthy();
    expect(screen.getAllByText(t("statistics.otherGroup")).length).toBe(2);
    expect(screen.getByRole("link", { name: t("statistics.dbipCredit") }).getAttribute("href")).toBe("https://db-ip.com");
    expect(screen.getByText(t("statistics.deviceSmartSpeaker"))).toBeTruthy();
    expect(screen.getByText("VLC")).toBeTruthy();
    expect(screen.getByRole("link", { name: t("statistics.csvPlayers") }).getAttribute("href")).toContain("by=player");
    // A period of its own: in the address.
    fireEvent.change(period, { target: { value: "90" } });
    await waitFor(() => expect(window.location.search).toBe("?days=90"));
    fireEvent.change(period, { target: { value: "30" } });
    await waitFor(() => expect(window.location.search).toBe(""));
  });

  it("leaves countries out without a GeoIP database", async () => {
    show("", undefined, { "GET /api/v1/stations/{station_id}/listeners/breakdown": { geoip: null, groups: [] } });
    expect(await screen.findByText(t("statistics.devicesPlayers"))).toBeTruthy();
    await waitFor(() => expect(screen.queryByText(t("statistics.countries"))).toBeNull());
  });

  it("says why the figures from sessions are missing", async () => {
    const none: Audience = { ...CURRENT, sessions: null, ttsl_hours: null, atsl_s: null, median_session_s: null, cume: null, listener_days: null, aqh: null, session_lengths: null };
    show("", { current: none, previous: none });
    expect(await screen.findByText(t("statistics.noAudience"))).toBeTruthy();
  });
});
