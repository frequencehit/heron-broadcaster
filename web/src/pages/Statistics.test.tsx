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

const ROUTES = ["audience", "days", "hours-of-day", "tracks", "players"] as const;

function show(search: string, audience: { current: Audience; previous: Audience } = { current: CURRENT, previous: PREVIOUS }) {
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
    "GET /api/v1/stations/{station_id}/listeners/days": [],
    "GET /api/v1/stations/{station_id}/listeners/hours-of-day": [
      { hour: 8, days: 30, average: 4, peak: 9, listening_hours: 4, tune_in: 1.5, tune_out: 0.5 },
    ],
    "GET /api/v1/stations/{station_id}/listeners/tracks": [],
    "GET /api/v1/stations/{station_id}/listeners/players": [],
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
    await waitFor(() => expect(query("players")).toBeTruthy());
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

  it("says why the figures from sessions are missing", async () => {
    const none: Audience = { ...CURRENT, sessions: null, ttsl_hours: null, atsl_s: null, median_session_s: null, cume: null, listener_days: null, aqh: null, session_lengths: null };
    show("", { current: none, previous: none });
    expect(await screen.findByText(t("statistics.noAudience"))).toBeTruthy();
  });
});
