// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { User } from "../api";
import { t } from "../i18n";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { DEFAULT_STATUS_BAR, type StatusBarSettings } from "../lib/statusbar";
import { answer, renderApp, sent, user } from "../test/render";
import { StatusBarSettingsCard } from "./StatusBarSettings";

const PREFERENCES = "PUT /api/v1/auth/preferences";

/** The API: the settings sent come back, a field left out taking its default. */
function api(modules = ALL_ON) {
  answer({
    "GET /api/v1/category-types": [
      { id: 1, name: "Song", categories: 3, no_two_in_a_row: false, plays_over_intro: false, sends_metadata: true },
      { id: 2, name: "Jingle", categories: 1, no_two_in_a_row: true, plays_over_intro: true, sends_metadata: false },
    ],
    "GET /api/v1/stations/{station_id}/listeners/now": { outputs: [], total: 3 },
    "GET /api/v1/stations/{station_id}/shows": [],
    "GET /api/v1/stations/{id}/modules": { on: modules, own: modules },
    [PREFERENCES]: (options: { body: { status_bar: Partial<StatusBarSettings> } }): User => ({
      ...user(),
      status_bar: { ...DEFAULT_STATUS_BAR, ...options.body.status_bar },
    }),
  });
}

const lastSaved = () => (sent(PREFERENCES).at(-1)?.options as { body: { status_bar: StatusBarSettings } }).body.status_bar;

describe("Account › Status bar", () => {
  beforeEach(() => {
    chooseStation(1);
    api();
  });

  it("previews the bar and saves each change at once, the preview following it", async () => {
    renderApp(<StatusBarSettingsCard />);
    const preview = () => document.querySelector(".statusbar-preview")!;
    await waitFor(() => expect(within(preview() as HTMLElement).getByText("listeners 3")).toBeTruthy());

    fireEvent.click(screen.getByRole("checkbox", { name: new RegExp(t("statusbarSettings.listeners")) }));
    expect(within(preview() as HTMLElement).queryByText(/^listeners/)).toBeNull();
    await waitFor(() => expect(sent(PREFERENCES)).toHaveLength(1));
    expect(lastSaved()).toEqual({ ...DEFAULT_STATUS_BAR, listeners: false });

    fireEvent.click(screen.getByRole("radio", { name: new RegExp(t("statusbarSettings.time.remaining_total")) }));
    await waitFor(() => expect(lastSaved().time).toBe("remaining_total"));
    expect(lastSaved().listeners).toBe(false);
  });

  it("shows the bar at a phone's width", () => {
    renderApp(<StatusBarSettingsCard />);
    const frame = document.querySelector<HTMLElement>(".statusbar-preview-frame > div")!;
    expect(frame.style.width).toBe("100%");
    fireEvent.click(screen.getByRole("button", { name: t("statusbarSettings.phone") }));
    expect(frame.style.width).toBe("390px");
  });

  it("chooses the history's category type, or any", async () => {
    renderApp(<StatusBarSettingsCard />);
    const type = (await screen.findByRole("option", { name: "Jingle" })).closest("select")!;
    expect(type.disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: new RegExp(t("statusbarSettings.history")) }));
    expect(type.disabled).toBe(false);
    fireEvent.change(type, { target: { value: "2" } });
    await waitFor(() => expect(lastSaved()).toMatchObject({ history: true, history_type_id: 2 }));
    fireEvent.change(type, { target: { value: "" } });
    await waitFor(() => expect(lastSaved().history_type_id).toBeNull());
  });

  it("saves the countdowns' seconds from 1 to 60 only, and whether they blink", async () => {
    renderApp(<StatusBarSettingsCard />);
    const seconds = screen.getByRole("spinbutton", {
      name: t("statusbarSettings.secondsOf", { name: t("statusbarSettings.toVocals") }),
    });
    fireEvent.change(seconds, { target: { value: "0" } });
    fireEvent.change(seconds, { target: { value: "61" } });
    expect(sent(PREFERENCES)).toHaveLength(0);
    fireEvent.change(seconds, { target: { value: "8" } });
    await waitFor(() => expect(lastSaved().intro_seconds).toBe(8));
    const blinks = screen.getAllByRole("checkbox", { name: t("statusbarSettings.blink") });
    fireEvent.click(blinks[1]!);
    await waitFor(() => expect(lastSaved()).toMatchObject({ intro_seconds: 8, end_blink: true }));
  });

  it("picks the colours, back to the defaults on demand", async () => {
    renderApp(<StatusBarSettingsCard />);
    fireEvent.change(screen.getByLabelText(t("statusbarSettings.introColor")), { target: { value: "#AA3300" } });
    await waitFor(() => expect(lastSaved().intro_color).toBe("#aa3300"));
    fireEvent.click(screen.getByRole("button", { name: t("statusbarSettings.defaultColor") }));
    await waitFor(() => expect(lastSaved().intro_color).toBeNull());
  });

  it("restores the defaults by sending none", async () => {
    renderApp(<StatusBarSettingsCard />, { ...user(), status_bar: { ...DEFAULT_STATUS_BAR, queue: true, time: "remaining" } });
    fireEvent.click(screen.getByRole("button", { name: t("statusbarSettings.reset") }));
    await waitFor(() => expect(lastSaved()).toEqual({}));
    await waitFor(() =>
      expect(
        (screen.getByRole("radio", { name: `${t("statusbarSettings.time.elapsed")}1:23` }) as HTMLInputElement)
          .checked,
      ).toBe(true),
    );
  });

  it("hides the bar, and says when an admin turned it off for the station", async () => {
    api({ ...ALL_ON, status_bar: false });
    renderApp(<StatusBarSettingsCard />);
    expect(await screen.findByText(t("statusbarSettings.moduleOff"))).toBeTruthy();
    fireEvent.click(screen.getByRole("checkbox", { name: t("statusbarSettings.show") }));
    expect(screen.getByText(t("statusbarSettings.hidden"))).toBeTruthy();
    await waitFor(() => expect(lastSaved().show).toBe(false));
  });
});
