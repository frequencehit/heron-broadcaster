// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { ModulesPage } from "../pages/Modules";
import { answer, renderApp, sent, user } from "../test/render";
import { CoverThumb } from "./Cover";
import { ModuleGate, ModuleSwitches } from "./Modules";

const STATION_MODULES = "GET /api/v1/stations/{id}/modules";

function stationModules(installation: Partial<typeof ALL_ON>, station: Partial<typeof ALL_ON> = {}) {
  const i = { ...ALL_ON, ...installation };
  const s = { ...ALL_ON, ...station };
  const on = { ...ALL_ON };
  for (const key of Object.keys(on) as (keyof typeof on)[]) {
    on[key] = i[key] && s[key];
  }
  return { installation: i, station: s, on };
}

describe("modules", () => {
  beforeEach(() => chooseStation(1));

  it("a module off for the installation cannot be turned on for a station", () => {
    const onChange = vi.fn();
    renderApp(
      <ModuleSwitches
        value={ALL_ON}
        installation={{ ...ALL_ON, dedications: true, match: true, top30: true, cover_art: false }}
        canEdit
        saving={false}
        onChange={onChange}
      />,
    );
    const cover = screen.getByRole("checkbox", { name: /Cover art/ }) as HTMLInputElement;
    expect(cover.checked).toBe(false);
    expect(cover.disabled).toBe(true);
    expect(screen.getByText(/Turned off for the installation/)).toBeTruthy();
    fireEvent.click(screen.getByRole("checkbox", { name: /Medleys/ }));
    expect(onChange).toHaveBeenCalledWith({ ...ALL_ON, medleys: false });
  });

  it("a page of a module turned off says so instead of failing", async () => {
    answer({ [STATION_MODULES]: stationModules({}, { medleys: false }) });
    renderApp(
      <ModuleGate module="medleys">
        <p>the medleys</p>
      </ModuleGate>,
    );
    expect(await screen.findByText("The Medleys module is turned off.")).toBeTruthy();
    expect(screen.queryByText("the medleys")).toBeNull();
    // Off for the station: turned on in its settings.
    expect(screen.getByRole("link", { name: "Settings › Station › Modules" })).toBeTruthy();
  });

  it("a page of a module that is on shows", async () => {
    answer({ [STATION_MODULES]: stationModules({}) });
    renderApp(
      <ModuleGate module="webhooks">
        <p>the webhooks</p>
      </ModuleGate>,
    );
    expect(await screen.findByText("the webhooks")).toBeTruthy();
  });

  it("covers are not shown while cover art is off", async () => {
    answer({ [STATION_MODULES]: stationModules({ cover_art: false }) });
    const { container } = renderApp(<CoverThumb url="/api/v1/public/covers/ab" />);
    await waitFor(() => expect(sent(STATION_MODULES)).toHaveLength(1));
    await waitFor(() => expect(container.querySelector(".cover-art")).toBeNull());
  });

  it("an admin turns a module off for the installation", async () => {
    answer({
      "GET /api/v1/modules": ALL_ON,
      "PUT /api/v1/modules": (options: { body: unknown }) => options.body,
    });
    renderApp(<ModulesPage />);
    fireEvent.click(await screen.findByRole("checkbox", { name: /Webhooks/ }));
    await waitFor(() => expect(sent("PUT /api/v1/modules")).toHaveLength(1));
    expect(sent("PUT /api/v1/modules")[0]?.options?.body).toEqual({ ...ALL_ON, webhooks: false });
  });

  it("only admins change the installation's modules", () => {
    renderApp(<ModulesPage />, user("operator"));
    expect(screen.getByText(/admins/i)).toBeTruthy();
  });
});
