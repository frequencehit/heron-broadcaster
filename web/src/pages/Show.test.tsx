// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import type { ShowSettings, ShowView } from "../lib/shows";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent, user } from "../test/render";
import { ShowPage } from "./Show";

const SHOW = "GET /api/v1/stations/{station_id}/shows/{module}";
const SAVE = "PUT /api/v1/stations/{station_id}/shows/{module}";

function settings(changes: Partial<ShowSettings> = {}): ShowSettings {
  return {
    enabled: true,
    hours: [{ days: [1, 2, 3, 4, 5], start_hour: 17, end_hour: 18 }],
    website: {
      title: "",
      color: "#c62828",
      small_widget: true,
      closed: "message",
      closed_message: "",
      page_url: null,
    },
    notifications: {
      webhook_ids: [],
      lead_minutes: 60,
      language: "en",
      events: {
        "show.upcoming": { enabled: true, message: "{name} opens at {next}: {link}" },
        "show.started": { enabled: true, message: "{name} is open until {end}" },
        "show.ended": { enabled: false, message: "{name} is closed" },
      },
    },
    ...changes,
  };
}

function view(module: string, changes: Partial<ShowSettings> = {}): ShowView {
  return {
    module: module as ShowView["module"],
    settings: settings(changes),
    status: { open: false, closes_at_unix_ms: null, next_opening_unix_ms: Date.UTC(2026, 10, 9, 16) },
    timezone: "Europe/Paris",
    webhooks: [{ id: 3, name: "Discord · #listeners" }],
  };
}

const MATCH = view("match", { hours: [{ days: [6], start_hour: 18, end_hour: 19 }] });

function answers(gatewayUrl: string | null = "https://public.radio.example.org") {
  answer({
    [SHOW]: view("dedications"),
    "GET /api/v1/stations/{station_id}/shows": [view("dedications"), MATCH],
    [SAVE]: (options: { body: ShowSettings }) => ({ ...view("dedications"), settings: options.body }),
    "GET /api/v1/stations": [{ id: 1, name: "Main", code: "main", enabled: true, position: 0 }],
    "GET /api/v1/stations/{station_id}/settings/station": {
      timezone: "Europe/Paris",
      public_url: null,
      gateway_url: gatewayUrl,
    },
    "GET /api/v1/stations/{id}/modules": {
      installation: { ...ALL_ON, dedications: true, match: true },
      station: { ...ALL_ON, dedications: true, match: true, top30: true },
      on: { ...ALL_ON, dedications: true, match: true },
    },
  });
}

describe("a show's page", () => {
  beforeEach(() => chooseStation(1));

  it("gives the lines for the website, from the gateway's address", async () => {
    answers();
    renderApp(<ShowPage module="dedications" />);
    expect(
      await screen.findByText(
        '<script src="https://public.radio.example.org/api/v1/public/stations/main/dedications.js" data-mode="page" async></script>',
      ),
    ).toBeTruthy();
    expect(screen.getByRole("link", { name: "View the page alone" }).getAttribute("href")).toBe(
      "https://public.radio.example.org/api/v1/public/stations/main/dedications.html",
    );
  });

  it("asks for the gateway's address when there is none", async () => {
    answers(null);
    renderApp(<ShowPage module="dedications" />);
    expect(await screen.findByText(/Set the station's gateway address/)).toBeTruthy();
  });

  it("says which hours another show has, then saves", async () => {
    answers();
    renderApp(<ShowPage module="dedications" />);
    await screen.findByText("Hours");
    fireEvent.click(screen.getByRole("button", { name: "Add a range" }));
    // The new range (Mon–Fri) and Saturday, which the match has from 18:00.
    const sat = screen.getAllByRole("button", { name: "Sat" });
    fireEvent.click(sat[sat.length - 1] as HTMLElement);
    const to = screen.getAllByRole("combobox").filter((select) => (select as HTMLSelectElement).value === "18");
    fireEvent.change(to[to.length - 1] as HTMLElement, { target: { value: "19" } });
    expect(await screen.findByText(/18:00–19:00 on Sat: already Match/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent(SAVE)).toHaveLength(1));
    const body = sent(SAVE)[0]?.options?.body as ShowSettings;
    expect(body.hours).toHaveLength(2);
    expect(body.hours[1]?.days).toContain(6);
  });

  it("previews the notifications with an example", async () => {
    answers();
    renderApp(<ShowPage module="dedications" />);
    expect(await screen.findByText("Dedications is open until 18:00")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox", { name: "At opening" }), { target: { value: "{name} {artist}" } });
    expect(await screen.findByText("Unknown placeholders: artist")).toBeTruthy();
  });

  it("previews the widget closed, with its message, or nothing", async () => {
    answers();
    renderApp(<ShowPage module="dedications" />);
    await screen.findByText("Hours");
    fireEvent.click(screen.getByRole("button", { name: "Closed" }));
    expect(screen.getByText("Closed for now")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show nothing" }));
    expect(screen.getByText("Nothing is shown on the website.")).toBeTruthy();
  });

  it("is read-only for viewers", async () => {
    answers();
    renderApp(<ShowPage module="dedications" />, user("viewer"));
    await screen.findByText("Hours");
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect((screen.getByRole("checkbox", { name: "Open during its hours" }) as HTMLInputElement).disabled).toBe(true);
  });

  it("has no hours nor notifications for likes, which are always open", async () => {
    answers();
    const likes = { ...view("likes", { hours: [] }), status: { open: true, closes_at_unix_ms: null, next_opening_unix_ms: null } };
    likes.settings.notifications.events = {};
    answer({
      [SHOW]: likes,
      "GET /api/v1/stations/{station_id}/shows": [likes],
      [SAVE]: (options: { body: ShowSettings }) => ({ ...likes, settings: options.body }),
      "GET /api/v1/stations": [{ id: 1, name: "Main", code: "main", enabled: true, position: 0 }],
      "GET /api/v1/stations/{station_id}/settings/station": { timezone: "Europe/Paris", public_url: null, gateway_url: null },
      "GET /api/v1/stations/{id}/modules": { installation: ALL_ON, station: ALL_ON, on: ALL_ON },
    });
    renderApp(<ShowPage module="likes" />);
    const open = await screen.findByRole("checkbox", { name: "Open to votes" });
    expect(screen.queryByText("Hours")).toBeNull();
    expect(screen.queryByText("Notifications")).toBeNull();
    expect(screen.getByText("Only the track playing can be voted on.")).toBeTruthy();
    fireEvent.click(open);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent(SAVE)).toHaveLength(1));
    expect((sent(SAVE)[0]?.options?.body as ShowSettings).enabled).toBe(false);
  });
});
